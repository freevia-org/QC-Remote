use super::executor::{
    effective_timeouts, error_code_for, execute, ChunkSink, ExecutionLimits, Failure, SinkClosed,
};
use super::fetch::{FetchFailure, FetchRequest, FetchedHop, HttpBody, HttpFetcher};
use super::policy::{
    cross_host_headers, forwardable_headers, is_public_address, normalize_allowlist_domain,
    path_shape, DeviceHeader, HeaderRefusal, Refusal, RelayPolicy,
};
use super::pump::{FixedClock, RelayPump, RelaySink, READS_PER_FRAME};
use super::request::{decode_request, DecodeError, HttpMethod, RequestKind};
use super::response::{error_code, forward_response};
use super::{Disposition, UpdaterRelay};
use crate::tray_settings::UpdaterRelaySettings;
use crate::worker::DeviceController;
use prost::Message as _;
use quadcortex::commands::OutboundMessage;
use quadcortex::proto::cortex_protobuf_v2 as pa;
use reqwest::Url;
use std::cell::Cell;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// Every credential-shaped value below is invented for these tests.
const SECRET_TOKEN: &str = "synthetic-bearer-token-0123456789";
const SECRET_SERIAL: &str = "synthetic-serial-0042";
const SECRET_QUERY: &str = "synthetic-query-secret-77";
// Longer than a path segment the log shape keeps, as a real token would be.
const SECRET_PATH: &str = "synthetic-path-secret-4711";
const CHECK_URL: &str = "https://cloud-api.neuraldsp.com/firmware/v1/latest";

fn device_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("X-Cloud-Api-Client-Serial-Number", SECRET_SERIAL),
        ("Authorization", SECRET_TOKEN),
        ("Content-Type", "application/json"),
    ]
}

/// A type-61 request frame as the unit would build it, from invented values.
fn request_frame(
    kind: i32,
    url: &str,
    updater_request_id: u64,
    headers: &[(&str, &str)],
    payload: Option<&str>,
) -> Vec<u8> {
    pa::UpdaterForwardMessage {
        action: pa::message_action::Enum::Update as i32,
        updater_request_id,
        request_id: None,
        request: Some(pa::updater_forward_message::Request::Request(
            pa::UpdaterForwardRequest {
                r#type: kind,
                url: url.to_owned(),
                timeout: 60_000,
                headers: headers
                    .iter()
                    .map(|(name, value)| pa::ProductForwardHeaderPair {
                        name: (*name).to_owned(),
                        value: (*value).to_owned(),
                    })
                    .collect(),
                payload: payload
                    .map(|text| pa::updater_forward_request::Payload::Payload(text.to_owned())),
            },
        )),
        response: None,
    }
    .encode_to_vec()
}

const CHECK: i32 = pa::updater_forward_request_type::Enum::Check as i32;
const DOWNLOAD_UPDATE: i32 = pa::updater_forward_request_type::Enum::DownloadUpdate as i32;

fn check_frame(id: u64) -> Vec<u8> {
    request_frame(CHECK, CHECK_URL, id, &device_headers(), None)
}

// ---- a scripted network ----------------------------------------------------

struct Hop {
    status: u16,
    location: Option<String>,
    content_length: Option<u64>,
    pieces: Vec<Result<Vec<u8>, FetchFailure>>,
}

impl Hop {
    fn ok(pieces: &[&[u8]]) -> Self {
        Self::status(200, pieces)
    }

    fn status(status: u16, pieces: &[&[u8]]) -> Self {
        Self {
            status,
            location: None,
            content_length: None,
            pieces: pieces.iter().map(|piece| Ok(piece.to_vec())).collect(),
        }
    }

    fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            location: Some(location.to_owned()),
            content_length: None,
            pieces: Vec::new(),
        }
    }
}

struct Seen {
    url: String,
    headers: Vec<(String, String)>,
}

#[derive(Default)]
struct Scripted {
    script: Mutex<VecDeque<Result<Hop, FetchFailure>>>,
    seen: Mutex<Vec<Seen>>,
}

impl Scripted {
    fn with(script: Vec<Result<Hop, FetchFailure>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

struct PieceBody(VecDeque<Result<Vec<u8>, FetchFailure>>);

impl HttpBody for PieceBody {
    fn next_chunk(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, FetchFailure> {
        match self.0.pop_front() {
            Some(Ok(bytes)) => Ok(Some(bytes)),
            Some(Err(failure)) => Err(failure),
            None => Ok(None),
        }
    }
}

impl HttpFetcher for Scripted {
    fn get(&self, request: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure> {
        self.seen.lock().unwrap().push(Seen {
            url: request.url.to_string(),
            headers: request
                .headers
                .iter()
                .map(|header| (header.name.clone(), header.value.clone()))
                .collect(),
        });
        match self.script.lock().unwrap().pop_front() {
            Some(Ok(hop)) => Ok(FetchedHop {
                status: hop.status,
                location: hop.location,
                content_length: hop.content_length,
                body: Box::new(PieceBody(hop.pieces.into())),
            }),
            Some(Err(failure)) => Err(failure),
            None => Err(FetchFailure::Io),
        }
    }
}

/// A body fed from the test, so a request can be held open for as long as the
/// test wants.
struct ChannelBody(mpsc::Receiver<Vec<u8>>);

impl HttpBody for ChannelBody {
    fn next_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, FetchFailure> {
        match self.0.recv_timeout(timeout) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(FetchFailure::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(None),
        }
    }
}

/// Serves every request with a 200 whose body the test feeds.
struct Held {
    bodies: Mutex<VecDeque<mpsc::Receiver<Vec<u8>>>>,
}

impl Held {
    fn with_bodies(count: usize) -> (Arc<Self>, Vec<mpsc::Sender<Vec<u8>>>) {
        let mut senders = Vec::new();
        let mut receivers = VecDeque::new();
        for _ in 0..count {
            let (sender, receiver) = mpsc::channel();
            senders.push(sender);
            receivers.push_back(receiver);
        }
        (
            Arc::new(Self {
                bodies: Mutex::new(receivers),
            }),
            senders,
        )
    }
}

impl HttpFetcher for Held {
    fn get(&self, _request: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure> {
        let body = self.bodies.lock().unwrap().pop_front();
        match body {
            Some(receiver) => Ok(FetchedHop {
                status: 200,
                location: None,
                content_length: None,
                body: Box::new(ChannelBody(receiver)),
            }),
            None => Err(FetchFailure::Io),
        }
    }
}

#[derive(Default)]
struct Collect {
    frames: Vec<(Vec<u8>, bool, Option<i32>)>,
    closed: bool,
}

impl ChunkSink for Collect {
    fn emit(
        &mut self,
        payload: Vec<u8>,
        last: bool,
        error_code: Option<i32>,
    ) -> Result<(), SinkClosed> {
        if self.closed {
            return Err(SinkClosed);
        }
        self.frames.push((payload, last, error_code));
        Ok(())
    }
}

fn run_with(
    fetcher: &Scripted,
    frame: &[u8],
    limits: &ExecutionLimits,
    extra_domains: &[String],
) -> (super::executor::Outcome, Collect) {
    let request = decode_request(frame).expect("frame decodes");
    let mut sink = Collect::default();
    let outcome = execute(
        fetcher,
        &RelayPolicy::new(extra_domains),
        &request,
        limits,
        &AtomicBool::new(false),
        &mut sink,
    );
    (outcome, sink)
}

fn small_chunks() -> ExecutionLimits {
    ExecutionLimits {
        chunk_bytes: 4,
        ..ExecutionLimits::default()
    }
}

// ---- decoding ---------------------------------------------------------------

#[test]
fn a_check_request_decodes_with_its_headers_and_timeout() {
    let request = decode_request(&check_frame(26)).unwrap();
    assert_eq!(request.updater_request_id, 26);
    assert_eq!(request.request_id, None);
    assert_eq!(request.kind, Some(RequestKind::Check));
    assert_eq!(request.url, CHECK_URL);
    assert_eq!(request.timeout_ms, 60_000);
    assert_eq!(request.headers.len(), 3);
    assert!(!request.has_payload);
    assert_eq!(RequestKind::Check.http_method(), HttpMethod::Get);
}

#[test]
fn every_request_type_is_a_get_and_only_the_image_is_an_image() {
    for wire in 0..=3 {
        let kind = RequestKind::from_wire(wire).unwrap();
        assert_eq!(kind.http_method(), HttpMethod::Get);
        assert_eq!(kind.is_image_download(), wire == 3);
    }
    assert_eq!(RequestKind::from_wire(4), None);
    assert_eq!(RequestKind::from_wire(-1), None);
}

#[test]
fn frames_that_are_not_requests_do_not_decode_into_one() {
    assert_eq!(
        decode_request(&[0xff, 0xff, 0xff]),
        Err(DecodeError::Malformed)
    );
    assert_eq!(
        decode_request(&vec![0; 70 * 1024]),
        Err(DecodeError::TooLarge)
    );
    let reply = forward_response(None, 5, vec![1], true, None);
    assert_eq!(
        decode_request(&reply.payload),
        Err(DecodeError::NotARequest)
    );
}

#[test]
fn a_decoded_request_prints_no_header_value_and_no_url() {
    let frame = request_frame(
        CHECK,
        &format!("{CHECK_URL}?sig={SECRET_QUERY}"),
        9,
        &device_headers(),
        None,
    );
    let request = decode_request(&frame).unwrap();
    let shown = format!("{request:?}");
    for secret in [SECRET_TOKEN, SECRET_SERIAL, SECRET_QUERY, "neuraldsp"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    assert!(
        shown.contains("Authorization"),
        "names stay visible: {shown}"
    );
}

// ---- reply encoding ---------------------------------------------------------

#[test]
fn the_reply_wire_form_is_pinned() {
    let reply = forward_response(Some(1), 2, vec![4], true, None);
    assert_eq!(reply.message_type, 61);
    assert_eq!(
        reply.payload,
        [0x08, 0x01, 0x10, 0x01, 0x18, 0x02, 0x2a, 0x05, 0x0a, 0x01, 0x04, 0x10, 0x01]
    );
    // Without a request id the field is absent; an error code is field 3 of the
    // response.
    let failed = forward_response(None, 2, Vec::new(), true, Some(403));
    assert_eq!(
        failed.payload,
        [0x08, 0x01, 0x18, 0x02, 0x2a, 0x05, 0x10, 0x01, 0x18, 0x93, 0x03]
    );
}

// ---- policy -----------------------------------------------------------------

fn check(policy: &RelayPolicy, url: &str) -> Result<(), Refusal> {
    policy.check_url(&Url::parse(url).unwrap())
}

#[test]
fn neuraldsp_com_and_its_subdomains_over_https_are_allowed_at_any_path() {
    let policy = RelayPolicy::new(&[]);
    for allowed in [
        CHECK_URL,
        "https://neuraldsp.com/",
        "https://neuraldsp.com/anything/at/all?x=1",
        "https://cloud-api.neuraldsp.com/logs/v1/upload",
        "https://downloads.neuraldsp.com/CorOS/image.bin",
        "https://a.b.c.neuraldsp.com/%2e%2e/x",
        "https://CLOUD-API.NEURALDSP.COM/firmware/x",
        "https://cloud-api.neuraldsp.com//double//slash",
    ] {
        assert_eq!(check(&policy, allowed), Ok(()), "{allowed}");
    }
}

#[test]
fn a_domain_matches_on_a_label_boundary_only() {
    let policy = RelayPolicy::new(&[]);
    for refused in [
        "https://evilneuraldsp.com/firmware/x",
        "https://xneuraldsp.com/",
        "https://not-neuraldsp.com/",
        "https://neuraldsp.com.evil.example/firmware/x",
        "https://cloud-api.neuraldsp.com.evil.test/firmware/x",
        "https://neuraldsp.co/",
        "https://neuraldsp.com./",
        "https://cloud-api.neuraldsp.com./firmware/x",
        "https://neuraldsp.org/",
        "https://example.net/neuraldsp.com",
        "https://evil.example/?host=cloud-api.neuraldsp.com",
    ] {
        assert!(
            matches!(check(&policy, refused), Err(Refusal::Host(_))),
            "{refused}"
        );
    }
    assert_eq!(
        check(&policy, "https://evilneuraldsp.com/x"),
        Err(Refusal::Host("evilneuraldsp.com".into()))
    );
}

#[test]
fn https_port_443_and_a_plain_name_stay_required_inside_the_allowed_domain() {
    let policy = RelayPolicy::new(&[]);
    for refused in [
        "http://cloud-api.neuraldsp.com/firmware/x",
        "ftp://cloud-api.neuraldsp.com/firmware/x",
        "https://user:pw@cloud-api.neuraldsp.com/firmware/x",
        "https://user@cloud-api.neuraldsp.com/firmware/x",
        "https://cloud-api.neuraldsp.com:8443/firmware/x",
        "https://neuraldsp.com:444/",
        "https://neuraldsp.com@evil.example/",
    ] {
        assert!(check(&policy, refused).is_err(), "{refused}");
    }
    assert_eq!(
        check(&policy, "https://cloud-api.neuraldsp.com:443/x"),
        Ok(())
    );
}

#[test]
fn a_request_for_another_domain_is_refused_and_named_without_path_or_query() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]);
    let frame = request_frame(
        CHECK,
        &format!("https://downloads.example.net/private/image.bin?token={SECRET_QUERY}"),
        4,
        &device_headers(),
        None,
    );
    let (outcome, sink) = run_with(&fetcher, &frame, &ExecutionLimits::default(), &[]);
    assert_eq!(fetcher.calls(), 0);
    assert_eq!(
        outcome.failure,
        Some(Failure::Refused("domain-not-allowed"))
    );
    assert_eq!(outcome.host, "downloads.example.net");
    assert_eq!(
        outcome.path_shape, "",
        "a refused domain is logged by host only"
    );
    assert_eq!(
        sink.frames,
        vec![(Vec::new(), true, Some(error_code::REFUSED))]
    );
}

#[test]
fn address_literals_in_every_spelling_are_refused() {
    let policy = RelayPolicy::new(&[
        "10.0.0.1".to_owned(),
        "[::1]".to_owned(),
        "127.1".to_owned(),
    ]);
    for literal in [
        "https://127.0.0.1/x",
        "https://127.1/x",
        "https://0x7f.1/x",
        "https://2130706433/x",
        "https://10.0.0.1/x",
        "https://169.254.169.254/latest/meta-data",
        "https://[::1]/x",
        "https://[::ffff:127.0.0.1]/x",
        "https://[fe80::1]/x",
    ] {
        assert_eq!(
            check(&policy, literal),
            Err(Refusal::IpLiteral),
            "{literal}"
        );
    }
}

#[test]
fn extra_domains_must_be_plain_names_and_never_widen_into_patterns() {
    for refused in [
        "",
        "localhost",
        "*.example.net",
        "example.net/path",
        "example.net:8443",
        "1.2.3.4",
        "127.1",
        "[::1]",
        "-bad.example.net",
        "bad-.example.net",
        "a..b.example.net",
        "user@example.net",
        "exa mple.net",
        "example.123",
    ] {
        assert_eq!(normalize_allowlist_domain(refused), None, "{refused:?}");
    }
    assert_eq!(
        normalize_allowlist_domain(" Downloads.Example.NET "),
        Some("downloads.example.net".to_owned())
    );
}

#[test]
fn an_extra_domain_is_allowed_with_its_subdomains_and_nothing_else() {
    let policy = RelayPolicy::new(&[
        "downloads.example.net".to_owned(),
        "DOWNLOADS.example.net".to_owned(),
        "*.example.org".to_owned(),
    ]);
    assert_eq!(policy.domains(), ["neuraldsp.com", "downloads.example.net"]);
    for allowed in [
        "https://downloads.example.net/image",
        "https://eu.downloads.example.net/any/path?x=1",
        "https://cloud-api.neuraldsp.com/x",
    ] {
        assert_eq!(check(&policy, allowed), Ok(()), "{allowed}");
    }
    for refused in [
        "https://other.example.net/image",
        "https://example.net/image",
        "https://notdownloads.example.net/image",
        "https://downloads.example.net.evil.example/image",
        "https://example.org/image",
    ] {
        assert!(check(&policy, refused).is_err(), "{refused}");
    }
}

#[test]
fn only_globally_routable_addresses_are_public() {
    let public = [
        "8.8.8.8",
        "1.1.1.1",
        "93.184.216.34",
        "2606:4700:4700::1111",
        "2a00:1450:4001:81b::200e",
    ];
    for address in public {
        assert!(
            is_public_address(address.parse::<IpAddr>().unwrap()),
            "{address}"
        );
    }
    let refused = [
        "0.0.0.0",
        "0.1.2.3",
        "127.0.0.1",
        "127.255.255.254",
        "10.1.2.3",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "100.127.255.254",
        "192.0.0.8",
        // Special-purpose anycast and relay ranges, first and last address.
        "192.88.99.0",
        "192.88.99.255",
        "192.31.196.1",
        "192.52.193.254",
        "192.175.48.0",
        "192.175.48.255",
        "192.0.2.1",
        "198.18.0.1",
        "198.19.255.255",
        "198.51.100.7",
        "203.0.113.9",
        "224.0.0.1",
        "239.255.255.250",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "fe80::1",
        "fc00::1",
        "fd12:3456::1",
        "fec0::1",
        "ff02::1",
        "2001:db8::1",
        "2001:0:4136:e378::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "::ffff:169.254.169.254",
        "64:ff9b::7f00:1",
        "64:ff9b::a00:1",
        "2002:7f00:1::1",
        "2002:c0a8:101::1",
        "::7f00:1",
        "100::1",
        // IPv4-translated (::ffff:0:a.b.c.d): not the mapped form, same risk.
        "::ffff:0:7f00:1",
        "::ffff:0:a00:1",
        "::ffff:0:a9fe:a9fe",
        "::ffff:0:c0a8:101",
        // Local-use NAT64 (64:ff9b:1::/48) and the 3fff::/20 documentation block.
        "64:ff9b:1::1",
        "64:ff9b:1:ffff::7f00:1",
        "3fff::1",
        "3fff:fff:ffff::1",
        // 2001::/23 (IETF protocol assignments, Teredo included) and 5f00::/16.
        "2001:1::1",
        "2001:2::1",
        "2001:3::1",
        "2001:4:112::1",
        "2001:10::1",
        "2001:20::1",
        "2001:1ff:ffff::1",
        "5f00::1",
        "5f00:ffff::1",
    ];
    for address in refused {
        assert!(
            !is_public_address(address.parse::<IpAddr>().unwrap()),
            "{address}"
        );
    }
    // The neighbours of those ranges are ordinary addresses.
    for address in [
        "192.88.98.255",
        "192.88.100.0",
        "192.31.195.255",
        "192.175.49.0",
    ] {
        assert!(
            is_public_address(address.parse::<IpAddr>().unwrap()),
            "{address}"
        );
    }
    // The IPv4 inside an IPv6 form decides, both ways.
    assert!(is_public_address("::ffff:8.8.8.8".parse().unwrap()));
    assert!(is_public_address("64:ff9b::808:808".parse().unwrap()));
    assert!(is_public_address("2002:808:808::1".parse().unwrap()));
    assert!(is_public_address("::ffff:0:808:808".parse().unwrap()));
    // The edges of the blocks just refused: outside them is public.
    assert!(is_public_address("3fff:1000::1".parse().unwrap()));
    assert!(is_public_address("2001:200::1".parse().unwrap()));
    assert!(is_public_address("2001:4860:4860::8888".parse().unwrap()));
    assert!(is_public_address("5f01::1".parse().unwrap()));
    assert!(is_public_address("5eff::1".parse().unwrap()));
}

fn header(name: &str, value: &str) -> DeviceHeader {
    DeviceHeader {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

#[test]
fn only_the_headers_the_unit_supplied_are_forwarded_minus_connection_headers() {
    let supplied = [
        header("Authorization", SECRET_TOKEN),
        header("Content-Type", "application/json"),
        header("Host", "evil.test"),
        header("host", "evil.test"),
        header("Content-Length", "99"),
        header("Connection", "upgrade"),
        header("Transfer-Encoding", "chunked"),
        header("Upgrade", "websocket"),
        header("Proxy-Authorization", "x"),
        header("TE", "trailers"),
        header("Expect", "100-continue"),
        header("Range", "bytes=0-99"),
    ];
    let kept = forwardable_headers(&supplied).unwrap();
    let names: Vec<&str> = kept.iter().map(|kept| kept.name.as_str()).collect();
    assert_eq!(names, ["Authorization", "Content-Type", "Range"]);
}

#[test]
fn malformed_or_oversized_header_sets_are_refused_whole() {
    assert_eq!(
        forwardable_headers(&[header("Bad Name", "x")]),
        Err(HeaderRefusal::BadName)
    );
    assert_eq!(
        forwardable_headers(&[header("", "x")]),
        Err(HeaderRefusal::BadName)
    );
    for value in ["a\r\nInjected: 1", "a\nb", "a\0b"] {
        assert_eq!(
            forwardable_headers(&[header("X-Test", value)]),
            Err(HeaderRefusal::BadValue)
        );
    }
    let many: Vec<DeviceHeader> = (0..33).map(|n| header(&format!("X-{n}"), "v")).collect();
    assert_eq!(forwardable_headers(&many), Err(HeaderRefusal::TooMany));
    let large = [header("X-Big", &"a".repeat(17 * 1024))];
    assert_eq!(forwardable_headers(&large), Err(HeaderRefusal::TooLarge));
}

#[test]
fn a_header_prints_its_name_and_length_but_never_its_value() {
    let shown = format!("{:?}", header("Authorization", SECRET_TOKEN));
    assert!(!shown.contains(SECRET_TOKEN), "{shown}");
    assert!(
        shown.contains("Authorization") && shown.contains("len=33"),
        "{shown}"
    );
}

#[test]
fn a_hop_to_another_host_carries_only_the_byte_range() {
    let supplied = [
        header("Authorization", SECRET_TOKEN),
        header("X-Cloud-Api-Client-Serial-Number", SECRET_SERIAL),
        header("range", "bytes=10-"),
    ];
    assert_eq!(
        cross_host_headers(&supplied),
        vec![header("range", "bytes=10-")]
    );
}

#[test]
fn a_logged_path_has_no_query_values_and_no_identifiers() {
    let url = Url::parse(&format!(
        "https://cloud-api.neuraldsp.com/firmware/v1/latest?sig={SECRET_QUERY}&expires=9"
    ))
    .unwrap();
    let shape = path_shape(&url);
    assert_eq!(shape, "/firmware/v1/latest?sig&expires");
    assert!(!shape.contains(SECRET_QUERY));
    let opaque =
        Url::parse("https://h.example.net/download/9f8e7d6c5b4a39281706f5e4d3c2b1a0/img.bin")
            .unwrap();
    assert_eq!(path_shape(&opaque), "/download/<opaque>/img.bin");
    assert_eq!(
        path_shape(&Url::parse("https://h.example.net").unwrap()),
        "/"
    );
}

// ---- the executor -----------------------------------------------------------

#[test]
fn a_check_reply_is_one_final_frame_with_the_body_and_no_error() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"{\"v\":1}"]))]);
    let (outcome, sink) = run_with(&fetcher, &check_frame(1), &ExecutionLimits::default(), &[]);
    assert_eq!(outcome.failure, None);
    assert_eq!(outcome.status, Some(200));
    assert_eq!(sink.frames, vec![(b"{\"v\":1}".to_vec(), true, None)]);
    assert_eq!(outcome.bytes_from_server, 7);
    assert_eq!(outcome.bytes_to_device, 7);
    assert_eq!(outcome.host, "cloud-api.neuraldsp.com");
    let seen = fetcher.seen.lock().unwrap();
    assert_eq!(seen[0].url, CHECK_URL);
    let names: Vec<&str> = seen[0]
        .headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "X-Cloud-Api-Client-Serial-Number",
            "Authorization",
            "Content-Type"
        ],
        "the executor passes exactly the unit's headers and adds none (the real \
         client's default Accept is added below this seam, see http.rs)"
    );
}

#[test]
fn a_download_is_chunked_and_only_the_last_chunk_is_marked() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"abc", b"defgh", b"ij"]))]);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 2, &[], None);
    let (outcome, sink) = run_with(&fetcher, &frame, &small_chunks(), &[]);
    assert_eq!(outcome.failure, None);
    assert_eq!(
        sink.frames,
        vec![
            (b"abcd".to_vec(), false, None),
            (b"efgh".to_vec(), false, None),
            (b"ij".to_vec(), true, None),
        ]
    );
    assert_eq!(outcome.bytes_to_device, 10);
}

#[test]
fn a_body_that_ends_on_a_chunk_boundary_still_ends_with_a_marked_chunk() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"abcdefgh"]))]);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 2, &[], None);
    let (_, sink) = run_with(&fetcher, &frame, &small_chunks(), &[]);
    assert_eq!(
        sink.frames,
        vec![
            (b"abcd".to_vec(), false, None),
            (b"efgh".to_vec(), true, None)
        ]
    );
}

#[test]
fn an_empty_body_is_one_empty_final_frame() {
    let fetcher = Scripted::with(vec![Ok(Hop::status(204, &[]))]);
    let (outcome, sink) = run_with(&fetcher, &check_frame(1), &ExecutionLimits::default(), &[]);
    assert_eq!(outcome.failure, None);
    assert_eq!(sink.frames, vec![(Vec::new(), true, None)]);
}

#[test]
fn a_declared_length_over_the_cap_is_refused_before_any_body_is_read() {
    let mut hop = Hop::ok(&[b"never read"]);
    hop.content_length = Some(101);
    let fetcher = Scripted::with(vec![Ok(hop)]);
    let limits = ExecutionLimits {
        max_image_bytes: 100,
        ..ExecutionLimits::default()
    };
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 3, &[], None);
    let (outcome, sink) = run_with(&fetcher, &frame, &limits, &[]);
    assert_eq!(outcome.failure, Some(Failure::TooLarge));
    assert_eq!(
        sink.frames,
        vec![(Vec::new(), true, Some(error_code::TOO_LARGE))]
    );
    assert_eq!(outcome.bytes_from_server, 0);
}

#[test]
fn a_body_that_outgrows_the_cap_without_declaring_a_length_is_cut_off() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[
        b"0123456789",
        b"0123456789",
        b"0123456789",
    ]))]);
    let limits = ExecutionLimits {
        max_image_bytes: 25,
        chunk_bytes: 8,
        ..ExecutionLimits::default()
    };
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 3, &[], None);
    let (outcome, sink) = run_with(&fetcher, &frame, &limits, &[]);
    assert_eq!(outcome.failure, Some(Failure::TooLarge));
    let last = sink.frames.last().unwrap();
    assert_eq!(last, &(Vec::new(), true, Some(error_code::TOO_LARGE)));
}

#[test]
fn a_small_request_has_the_small_cap_not_the_image_cap() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[&[0_u8; 64]]))]);
    let limits = ExecutionLimits {
        max_small_bytes: 32,
        ..ExecutionLimits::default()
    };
    let (outcome, _) = run_with(&fetcher, &check_frame(1), &limits, &[]);
    assert_eq!(outcome.failure, Some(Failure::TooLarge));
}

#[test]
fn a_server_error_is_passed_on_with_its_status_and_a_bounded_body() {
    let fetcher = Scripted::with(vec![Ok(Hop::status(404, &[b"not ", b"found"]))]);
    let (outcome, sink) = run_with(&fetcher, &check_frame(1), &ExecutionLimits::default(), &[]);
    assert_eq!(outcome.failure, Some(Failure::HttpStatus(404)));
    assert_eq!(sink.frames, vec![(b"not found".to_vec(), true, Some(404))]);

    let fetcher = Scripted::with(vec![Ok(Hop::status(500, &[&[7_u8; 100]]))]);
    let limits = ExecutionLimits {
        error_body_bytes: 10,
        ..ExecutionLimits::default()
    };
    let (_, sink) = run_with(&fetcher, &check_frame(1), &limits, &[]);
    assert_eq!(sink.frames, vec![(vec![7_u8; 10], true, Some(500))]);
}

#[test]
fn a_domain_off_the_allowlist_is_refused_without_a_request_and_named() {
    let fetcher = Scripted::with(vec![]);
    let frame = request_frame(
        CHECK,
        &format!("https://downloads.example.net/image.bin?sig={SECRET_QUERY}"),
        4,
        &device_headers(),
        None,
    );
    let (outcome, sink) = run_with(&fetcher, &frame, &ExecutionLimits::default(), &[]);
    assert_eq!(
        fetcher.calls(),
        0,
        "nothing may be sent to a host that is not allowed"
    );
    assert_eq!(
        outcome.failure,
        Some(Failure::Refused("domain-not-allowed"))
    );
    assert_eq!(outcome.host, "downloads.example.net");
    assert_eq!(outcome.path_shape, "");
    assert_eq!(
        sink.frames,
        vec![(Vec::new(), true, Some(error_code::REFUSED))]
    );
}

#[test]
fn a_domain_the_user_added_is_allowed() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]);
    let frame = request_frame(
        CHECK,
        "https://downloads.example.net/image.bin",
        4,
        &[],
        None,
    );
    let (outcome, _) = run_with(
        &fetcher,
        &frame,
        &ExecutionLimits::default(),
        &["downloads.example.net".to_owned()],
    );
    assert_eq!(outcome.failure, None);
}

#[test]
fn refused_urls_never_reach_the_network() {
    for url in [
        "http://cloud-api.neuraldsp.com/firmware/x",
        "https://127.0.0.1/x",
        "https://user:pw@cloud-api.neuraldsp.com/firmware/x",
        "https://cloud-api.neuraldsp.com:444/firmware/x",
        "not a url",
        "",
    ] {
        let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]);
        let frame = request_frame(CHECK, url, 4, &[], None);
        let (outcome, sink) = run_with(&fetcher, &frame, &ExecutionLimits::default(), &[]);
        assert_eq!(fetcher.calls(), 0, "{url}");
        assert!(
            matches!(outcome.failure, Some(Failure::Refused(_))),
            "{url}"
        );
        assert_eq!(sink.frames.len(), 1, "{url}");
        assert_eq!(sink.frames[0].2, Some(error_code::REFUSED), "{url}");
    }
}

#[test]
fn a_request_with_a_body_or_an_unknown_type_is_not_sent() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]);
    let with_body = request_frame(CHECK, CHECK_URL, 5, &[], Some("{\"a\":1}"));
    let (outcome, sink) = run_with(&fetcher, &with_body, &ExecutionLimits::default(), &[]);
    assert_eq!(
        outcome.failure,
        Some(Failure::BadRequest("request-has-body"))
    );
    assert_eq!(sink.frames[0].2, Some(error_code::BAD_REQUEST));
    // An empty body string is no body.
    let empty_body = request_frame(CHECK, CHECK_URL, 5, &[], Some(""));
    let (outcome, _) = run_with(&fetcher, &empty_body, &ExecutionLimits::default(), &[]);
    assert_eq!(outcome.failure, None);

    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]);
    let unknown = request_frame(9, CHECK_URL, 5, &[], None);
    let (outcome, _) = run_with(&fetcher, &unknown, &ExecutionLimits::default(), &[]);
    assert_eq!(
        outcome.failure,
        Some(Failure::BadRequest("request-type-unknown"))
    );
    assert_eq!(fetcher.calls(), 0);

    let bad_header = request_frame(CHECK, CHECK_URL, 5, &[("X-A", "a\r\nb: c")], None);
    let (outcome, _) = run_with(&fetcher, &bad_header, &ExecutionLimits::default(), &[]);
    assert_eq!(
        outcome.failure,
        Some(Failure::BadRequest("headers-refused"))
    );
    assert_eq!(fetcher.calls(), 0);
}

#[test]
fn a_redirect_to_an_allowed_host_is_followed_and_checked_again() {
    let fetcher = Scripted::with(vec![
        Ok(Hop::redirect("/firmware/v1/other")),
        Ok(Hop::ok(&[b"done"])),
    ]);
    let (outcome, sink) = run_with(&fetcher, &check_frame(1), &ExecutionLimits::default(), &[]);
    assert_eq!(outcome.failure, None);
    assert_eq!(outcome.hops, 2);
    assert_eq!(sink.frames, vec![(b"done".to_vec(), true, None)]);
    let seen = fetcher.seen.lock().unwrap();
    assert_eq!(
        seen[1].url,
        "https://cloud-api.neuraldsp.com/firmware/v1/other"
    );
    assert_eq!(
        seen[1].headers, seen[0].headers,
        "same host keeps the unit's headers"
    );
}

#[test]
fn a_redirect_off_the_allowlist_or_off_https_is_refused_at_that_hop() {
    for target in [
        "https://evil.example.net/x",
        "http://cloud-api.neuraldsp.com/firmware/x",
        "https://127.0.0.1/x",
        "https://169.254.169.254/latest",
        "//evil.example.net/x",
    ] {
        let fetcher = Scripted::with(vec![Ok(Hop::redirect(target)), Ok(Hop::ok(&[b"x"]))]);
        let (outcome, sink) = run_with(&fetcher, &check_frame(1), &ExecutionLimits::default(), &[]);
        assert_eq!(
            fetcher.calls(),
            1,
            "{target}: the second hop must not be requested"
        );
        assert!(
            matches!(outcome.failure, Some(Failure::Refused(_))),
            "{target}"
        );
        assert_eq!(
            sink.frames.last().unwrap().2,
            Some(error_code::REFUSED),
            "{target}"
        );
    }
}

#[test]
fn redirects_are_capped_at_three_and_every_hop_is_requested_once() {
    let script = (0..6)
        .map(|n| Ok(Hop::redirect(&format!("/firmware/v1/hop/{n}"))))
        .collect();
    let fetcher = Scripted::with(script);
    let (outcome, sink) = run_with(&fetcher, &check_frame(1), &ExecutionLimits::default(), &[]);
    assert_eq!(outcome.failure, Some(Failure::TooManyRedirects));
    assert_eq!(fetcher.calls(), 4, "the request and three redirects");
    assert_eq!(
        sink.frames,
        vec![(Vec::new(), true, Some(error_code::TOO_MANY_REDIRECTS))]
    );
}

#[test]
fn a_redirect_without_a_location_is_a_protocol_failure() {
    let mut hop = Hop::redirect("/x");
    hop.location = None;
    let fetcher = Scripted::with(vec![Ok(hop)]);
    let (outcome, sink) = run_with(&fetcher, &check_frame(1), &ExecutionLimits::default(), &[]);
    assert_eq!(
        outcome.failure,
        Some(Failure::Network(FetchFailure::Protocol))
    );
    assert_eq!(sink.frames[0].2, Some(error_code::PROTOCOL));
}

#[test]
fn a_hop_to_another_allowed_host_drops_the_credentials_but_keeps_the_range() {
    let fetcher = Scripted::with(vec![
        Ok(Hop::redirect(
            "https://downloads.example.net/image.bin?part=1",
        )),
        Ok(Hop::ok(&[b"bytes"])),
    ]);
    let headers = [
        ("Authorization", SECRET_TOKEN),
        ("X-Cloud-Api-Client-Serial-Number", SECRET_SERIAL),
        ("Range", "bytes=0-4"),
    ];
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 6, &headers, None);
    let (outcome, _) = run_with(
        &fetcher,
        &frame,
        &ExecutionLimits::default(),
        &["downloads.example.net".to_owned()],
    );
    assert_eq!(outcome.failure, None);
    let seen = fetcher.seen.lock().unwrap();
    assert_eq!(seen[0].headers.len(), 3);
    assert_eq!(
        seen[1].headers,
        vec![("Range".to_owned(), "bytes=0-4".to_owned())]
    );
    assert_eq!(
        seen[1].url,
        "https://downloads.example.net/image.bin?part=1"
    );
}

#[test]
fn a_hop_to_another_neuraldsp_subdomain_drops_the_credentials_too() {
    let fetcher = Scripted::with(vec![
        Ok(Hop::redirect(
            "https://downloads.neuraldsp.com/CorOS/image.bin",
        )),
        Ok(Hop::ok(&[b"bytes"])),
    ]);
    let headers = [
        ("Authorization", SECRET_TOKEN),
        ("X-Cloud-Api-Client-Serial-Number", SECRET_SERIAL),
        ("Range", "bytes=0-4"),
    ];
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 6, &headers, None);
    let (outcome, _) = run_with(&fetcher, &frame, &ExecutionLimits::default(), &[]);
    assert_eq!(outcome.failure, None);
    let seen = fetcher.seen.lock().unwrap();
    assert_eq!(
        seen[1].headers,
        vec![("Range".to_owned(), "bytes=0-4".to_owned())]
    );
}

#[test]
fn network_failures_map_to_status_shaped_error_codes() {
    let cases = [
        (FetchFailure::Timeout, error_code::TIMEOUT),
        (FetchFailure::Protocol, error_code::PROTOCOL),
        (FetchFailure::BlockedAddress, error_code::NETWORK),
        (FetchFailure::Dns, error_code::NETWORK),
        (FetchFailure::Connect, error_code::NETWORK),
        (FetchFailure::Tls, error_code::NETWORK),
        (FetchFailure::Io, error_code::NETWORK),
    ];
    for (failure, expected) in cases {
        let fetcher = Scripted::with(vec![Err(failure)]);
        let (outcome, sink) = run_with(&fetcher, &check_frame(1), &ExecutionLimits::default(), &[]);
        assert!(outcome.failure.is_some(), "{failure:?}");
        assert_eq!(
            sink.frames,
            vec![(Vec::new(), true, Some(expected))],
            "{failure:?}"
        );
    }
    assert_eq!(error_code_for(&Failure::HttpStatus(418)), 418);
}

#[test]
fn a_failure_in_the_middle_of_a_body_ends_with_an_error_frame_and_no_retry() {
    let hop = Hop {
        status: 200,
        location: None,
        content_length: None,
        pieces: vec![Ok(b"abcdefgh".to_vec()), Err(FetchFailure::Io)],
    };
    let fetcher = Scripted::with(vec![Ok(hop), Ok(Hop::ok(&[b"retry"]))]);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 7, &[], None);
    let (outcome, sink) = run_with(&fetcher, &frame, &small_chunks(), &[]);
    assert_eq!(fetcher.calls(), 1, "a failed transfer is never retried");
    assert_eq!(outcome.failure, Some(Failure::Network(FetchFailure::Io)));
    assert_eq!(
        sink.frames.last().unwrap(),
        &(Vec::new(), true, Some(error_code::NETWORK))
    );
}

#[test]
fn the_units_timeout_is_clamped_and_only_the_image_may_outlast_it() {
    let limits = ExecutionLimits::default();
    let second = Duration::from_secs(1);
    // Per-wait bound: the unit's value, clamped to [min, max].
    assert_eq!(
        effective_timeouts(RequestKind::Check, 60_000, &limits).0,
        Duration::from_secs(60)
    );
    assert_eq!(effective_timeouts(RequestKind::Check, 0, &limits).0, second);
    assert_eq!(
        effective_timeouts(RequestKind::Check, -5, &limits).0,
        second
    );
    assert_eq!(
        effective_timeouts(RequestKind::Check, i32::MAX, &limits).0,
        limits.max_timeout
    );
    // Total: the unit's value for small requests, the image limit for the image.
    assert_eq!(
        effective_timeouts(RequestKind::Check, 60_000, &limits).1,
        Duration::from_secs(60)
    );
    assert_eq!(
        effective_timeouts(RequestKind::DownloadChangelog, 90_000, &limits).1,
        Duration::from_secs(90)
    );
    assert_eq!(
        effective_timeouts(RequestKind::DownloadUpdate, 60_000, &limits),
        (Duration::from_secs(60), limits.image_overall)
    );
}

#[test]
fn a_spent_overall_deadline_stops_the_request_before_any_hop() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]);
    let limits = ExecutionLimits {
        small_overall: Duration::ZERO,
        ..ExecutionLimits::default()
    };
    let (outcome, sink) = run_with(&fetcher, &check_frame(1), &limits, &[]);
    assert_eq!(outcome.failure, Some(Failure::Timeout));
    assert_eq!(fetcher.calls(), 0);
    assert_eq!(
        sink.frames,
        vec![(Vec::new(), true, Some(error_code::TIMEOUT))]
    );
}

#[test]
fn a_cancelled_request_stops_quietly() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]);
    let request = decode_request(&check_frame(1)).unwrap();
    let mut sink = Collect::default();
    let cancel = AtomicBool::new(true);
    let outcome = execute(
        &*fetcher,
        &RelayPolicy::new(&[]),
        &request,
        &ExecutionLimits::default(),
        &cancel,
        &mut sink,
    );
    assert_eq!(outcome.failure, Some(Failure::Cancelled));
    assert_eq!(fetcher.calls(), 0);
    assert!(sink.frames.is_empty());
}

#[test]
fn a_device_that_stops_listening_ends_the_request_without_a_reply() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"abcdefgh"]))]);
    let request = decode_request(&request_frame(DOWNLOAD_UPDATE, CHECK_URL, 8, &[], None)).unwrap();
    let mut sink = Collect {
        closed: true,
        ..Collect::default()
    };
    let outcome = execute(
        &*fetcher,
        &RelayPolicy::new(&[]),
        &request,
        &small_chunks(),
        &AtomicBool::new(false),
        &mut sink,
    );
    assert_eq!(outcome.failure, Some(Failure::DeviceGone));
    assert!(sink.frames.is_empty());
}

// ---- the relay: switch, limit, status, privacy ------------------------------

fn decode_reply(
    message: &OutboundMessage,
) -> (u64, Option<u64>, Vec<u8>, Option<bool>, Option<i32>) {
    assert_eq!(message.message_type, 61);
    let decoded = pa::UpdaterForwardMessage::decode(message.payload.as_slice()).unwrap();
    let Some(pa::updater_forward_message::Response::Response(response)) = decoded.response else {
        panic!("a reply carries a response");
    };
    (
        decoded.updater_request_id,
        decoded
            .request_id
            .map(|pa::updater_forward_message::RequestId::RequestId(id)| id),
        response.payload,
        response
            .is_last_chunk
            .map(|pa::updater_forward_response::IsLastChunk::IsLastChunk(last)| last),
        response
            .error_code
            .map(|pa::updater_forward_response::ErrorCode::ErrorCode(code)| code),
    )
}

/// Everything the relay has to send until it is idle again.
fn drain(relay: &UpdaterRelay) -> Vec<OutboundMessage> {
    let started = Instant::now();
    let mut frames = Vec::new();
    loop {
        match relay.poll_outbound() {
            Some(frame) => frames.push(frame),
            None if !relay.is_active() => return frames,
            None => thread::sleep(Duration::from_millis(1)),
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the relay never went idle"
        );
    }
}

fn relay_with(fetcher: Arc<dyn HttpFetcher>) -> UpdaterRelay {
    UpdaterRelay::with_parts(fetcher, Arc::new(AtomicBool::new(false)))
}

fn disabled() -> UpdaterRelaySettings {
    UpdaterRelaySettings {
        enabled: false,
        ..UpdaterRelaySettings::default()
    }
}

#[test]
fn the_relay_is_on_by_default() {
    assert!(UpdaterRelaySettings::default().enabled);
    let relay = relay_with(Scripted::with(vec![]));
    let status = relay.status();
    assert!(status.enabled);
    assert_eq!(status.allowed_domains, ["neuraldsp.com"]);
    assert_eq!(status.note, None);
    assert!(
        DeviceController::start_disconnected()
            .unwrap()
            .status()
            .updater_relay
            .enabled
    );
}

#[test]
fn an_on_by_default_relay_answers_a_check_out_of_the_box() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"{\"latest\":\"4.2.0\"}"]))]);
    let relay = relay_with(fetcher.clone());
    let started = relay.on_forward_frame(&check_frame(26), Instant::now());
    assert_eq!(started, Disposition::Started);
    let frames = drain(&relay);
    assert_eq!(frames.len(), 1);
    let (updater_id, request_id, payload, last, error) = decode_reply(&frames[0]);
    assert_eq!(updater_id, 26);
    assert_eq!(request_id, None);
    assert_eq!(payload, b"{\"latest\":\"4.2.0\"}");
    assert_eq!(last, Some(true));
    assert_eq!(error, None);
    let status = relay.status();
    assert_eq!(status.requests_seen, 1);
    assert_eq!(status.requests_completed, 1);
    assert_eq!(status.bytes_to_device, 18);
    assert_eq!(status.recent.len(), 1);
    assert_eq!(status.recent[0].outcome, "completed");
    assert!(!status.in_flight);
}

#[test]
fn a_disabled_relay_drops_the_request_and_reports_that_the_unit_asked() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]);
    let relay = relay_with(fetcher.clone());
    relay.use_fixed_settings(disabled());
    let outcome = relay.on_forward_frame(&check_frame(26), Instant::now());
    assert_eq!(outcome, Disposition::Disabled);
    assert_eq!(
        fetcher.calls(),
        0,
        "a disabled relay never touches the network"
    );
    assert!(relay.poll_outbound().is_none(), "and never answers");
    assert!(!relay.is_active());
    let status = relay.status();
    assert!(!status.enabled);
    assert_eq!(status.requests_seen, 1);
    assert_eq!(status.requests_while_disabled, 1);
    assert_eq!(status.last_disabled_request_kind.as_deref(), Some("check"));
    assert_eq!(
        status.last_disabled_request_host.as_deref(),
        Some("cloud-api.neuraldsp.com")
    );
    assert!(status.last_disabled_request_at_unix_ms.is_some());
    assert!(status.note.as_deref().unwrap().contains("turned off"));
    assert!(
        relay.take_events().is_empty(),
        "nothing was run, so nothing is logged"
    );
}

#[test]
fn turning_the_relay_off_restores_the_drop_and_report_behaviour() {
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[b"one"])), Ok(Hop::ok(&[b"two"]))]);
    let relay = relay_with(fetcher.clone());
    assert_eq!(
        relay.on_forward_frame(&check_frame(1), Instant::now()),
        Disposition::Started
    );
    drain(&relay);
    assert_eq!(relay.status().requests_while_disabled, 0);

    relay.use_fixed_settings(disabled());
    assert_eq!(
        relay.on_forward_frame(&check_frame(2), Instant::now()),
        Disposition::Disabled
    );
    assert!(relay.poll_outbound().is_none());
    assert_eq!(fetcher.calls(), 1);
    assert_eq!(relay.status().requests_while_disabled, 1);

    relay.use_fixed_settings(UpdaterRelaySettings::default());
    assert_eq!(
        relay.on_forward_frame(&check_frame(3), Instant::now()),
        Disposition::Started
    );
    drain(&relay);
    assert_eq!(fetcher.calls(), 2);
    assert!(
        relay.status().note.is_none(),
        "an on relay has nothing to explain"
    );
}

#[test]
fn the_switch_is_read_from_the_settings_file_and_follows_it() {
    let directory =
        std::env::temp_dir().join(format!("qc-updater-relay-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let relay = relay_with(Scripted::with(vec![]));
    relay.set_settings_directory(directory.clone());
    assert!(relay.status().enabled, "no file means the default, on");

    crate::tray_settings::set_updater_relay_enabled_in(&directory, false).unwrap();
    // The cached value is re-read at most every few seconds; a new directory
    // assignment drops the cache, as a restart would.
    relay.set_settings_directory(directory.clone());
    assert!(!relay.status().enabled);
    assert_eq!(
        relay.on_forward_frame(&check_frame(1), Instant::now()),
        Disposition::Disabled
    );

    crate::tray_settings::set_updater_relay_enabled_in(&directory, true).unwrap();
    relay.set_settings_directory(directory.clone());
    assert!(relay.status().enabled);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_settings_file_that_cannot_be_read_keeps_the_last_known_switch() {
    use crate::tray_settings::SETTINGS_FILE;
    let directory = std::env::temp_dir().join(format!(
        "qc-updater-relay-unreadable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join(SETTINGS_FILE);
    let relay = relay_with(Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]));
    std::fs::write(&file, br#"{"updaterRelayEnabled": false}"#).unwrap();
    relay.set_settings_directory(directory.clone());
    let t0 = Instant::now();
    assert_eq!(
        relay.on_forward_frame(&check_frame(1), t0),
        Disposition::Disabled
    );

    // The file is there but cannot be read (here: a directory in its place,
    // as a lock held by a scanner would be). The switch stays off.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert_eq!(
        relay.on_forward_frame(&check_frame(2), t0 + Duration::from_secs(3)),
        Disposition::Disabled,
        "an unreadable file must not turn the relay back on"
    );
    // So does a file that is damaged.
    std::fs::remove_dir(&file).unwrap();
    std::fs::write(&file, b"{\"updaterRelayEnabled\": fal").unwrap();
    assert_eq!(
        relay.on_forward_frame(&check_frame(3), t0 + Duration::from_secs(6)),
        Disposition::Disabled,
        "a damaged file must not turn the relay back on"
    );
    assert_eq!(relay.status().requests_while_disabled, 3);

    // A file that is readable again is believed, on or off.
    std::fs::write(&file, br#"{"updaterRelayEnabled": true}"#).unwrap();
    assert_eq!(
        relay.on_forward_frame(&check_frame(4), t0 + Duration::from_secs(9)),
        Disposition::Started
    );
    drain(&relay);
    std::fs::write(&file, br#"{"updaterRelayEnabled": false}"#).unwrap();
    assert_eq!(
        relay.on_forward_frame(&check_frame(5), t0 + Duration::from_secs(12)),
        Disposition::Disabled
    );
    // Only an absent file is the default, on.
    std::fs::remove_file(&file).unwrap();
    let second = relay_with(Scripted::with(vec![Ok(Hop::ok(&[b"y"]))]));
    second.set_settings_directory(directory.clone());
    assert_eq!(
        second.on_forward_frame(&check_frame(6), Instant::now()),
        Disposition::Started
    );
    drain(&second);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_first_read_that_fails_leaves_the_default_on() {
    use crate::tray_settings::SETTINGS_FILE;
    // Nothing was ever read, so there is no earlier value to keep and the
    // documented default, on, applies.
    let directory = std::env::temp_dir().join(format!(
        "qc-updater-relay-first-read-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join(SETTINGS_FILE)).unwrap();
    let relay = relay_with(Scripted::with(vec![]));
    relay.set_settings_directory(directory.clone());
    assert!(relay.status().enabled);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_controller_status_carries_the_relay_and_serializes_it() {
    let controller = DeviceController::start_disconnected().unwrap();
    controller.updater_relay().use_fixed_settings(disabled());
    controller
        .updater_relay()
        .on_forward_frame(&check_frame(1), Instant::now());
    let status = controller.status();
    assert!(!status.updater_relay.enabled);
    assert_eq!(status.updater_relay.requests_while_disabled, 1);
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["updaterRelay"]["enabled"], false);
    assert_eq!(json["updaterRelay"]["requestsWhileDisabled"], 1);
    // The note, the host the unit asked for and the other detail stay in the
    // struct for the broker's own use and are not in the JSON (see below).
    assert!(json["updaterRelay"].get("note").is_none());
    assert!(json["updaterRelay"]
        .get("lastDisabledRequestHost")
        .is_none());
}

#[test]
fn the_status_an_ai_app_can_read_carries_no_host_path_audit_or_domain_list() {
    // `system.status` backs the `qc://status` resource, so everything in the
    // relay's part of it is readable by an attached AI app. After a completed
    // request, a refused one, a failed one and a request while off, only
    // counts, the switch, whether one is running and the last outcome word
    // may be there.
    let fetcher = Scripted::with(vec![
        Ok(Hop::ok(&[b"ok"])),
        Ok(Hop::redirect(
            "https://cdn.neuraldsp.com/img/x.bin?sig=SECRETQ",
        )),
        Err(FetchFailure::Tls),
    ]);
    let relay = relay_with(fetcher);
    relay.use_fixed_settings(UpdaterRelaySettings {
        extra_domains: vec!["downloads.example.net".to_owned()],
        ..UpdaterRelaySettings::default()
    });
    for (id, url) in [
        (1_u64, CHECK_URL),
        (2, "https://other.example.org/private/path?token=SECRETQ"),
        (3, "https://cloud-api.neuraldsp.com/firmware/v1/download"),
    ] {
        relay.on_forward_frame(
            &request_frame(CHECK, url, id, &device_headers(), None),
            Instant::now(),
        );
        drain(&relay);
    }
    relay.use_fixed_settings(disabled());
    relay.on_forward_frame(&check_frame(4), Instant::now());
    let status = relay.status();
    // The struct still holds the detail the broker's own code and tests use.
    assert_eq!(status.recent.len(), 3);
    assert_eq!(
        status.last_refused_host.as_deref(),
        Some("other.example.org")
    );
    assert!(status.last_disabled_request_host.is_some());
    assert!(status.note.is_some());

    let json = serde_json::to_value(&status).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "bytesFromServer",
            "bytesToDevice",
            "enabled",
            "inFlight",
            "lastOutcome",
            "refusedDomainRequests",
            "requestsCompleted",
            "requestsFailed",
            "requestsRefused",
            "requestsSeen",
            "requestsWhileDisabled"
        ]
    );
    let text = json.to_string();
    for place in [
        "neuraldsp",
        "example",
        "firmware",
        "private",
        "SECRETQ",
        "token",
        "check",
    ] {
        assert!(
            !text.contains(place),
            "{place} is in the status JSON: {text}"
        );
    }
    assert_eq!(json["lastOutcome"], "network:tls");
    assert_eq!(json["requestsSeen"], 4);
    assert_eq!(json["requestsWhileDisabled"], 1);
    assert_eq!(json["refusedDomainRequests"], 1);
}

#[test]
fn a_second_request_is_refused_while_one_is_running_and_a_repeat_is_ignored() {
    let (fetcher, senders) = Held::with_bodies(1);
    let relay = relay_with(fetcher);
    assert_eq!(
        relay.on_forward_frame(&check_frame(1), Instant::now()),
        Disposition::Started
    );
    assert_eq!(
        relay.on_forward_frame(&check_frame(1), Instant::now()),
        Disposition::Duplicate
    );
    assert_eq!(
        relay.on_forward_frame(&check_frame(2), Instant::now()),
        Disposition::Busy
    );
    let busy = relay.poll_outbound().expect("the refusal is sent at once");
    let (updater_id, _, payload, last, error) = decode_reply(&busy);
    assert_eq!(
        (updater_id, payload.is_empty(), last, error),
        (2, true, Some(true), Some(error_code::BUSY))
    );

    senders[0].send(b"body".to_vec()).unwrap();
    drop(senders);
    let frames = drain(&relay);
    assert_eq!(frames.len(), 1);
    assert_eq!(decode_reply(&frames[0]).2, b"body");
    let status = relay.status();
    assert_eq!(status.requests_completed, 1);
    assert_eq!(status.requests_refused, 1);
}

#[test]
fn the_units_next_request_is_not_turned_away_while_the_last_frames_are_still_queued() {
    // The unit answers a finished reply with its next request as soon as the
    // last frame is on the wire. The request used to count as running until
    // the loop's next poll noticed the closed queue, so that next request met
    // BUSY. Here the request is over once its last frame is queued.
    let limiter = Arc::new(AtomicBool::new(false));
    let big = vec![1_u8; 40_000];
    let fetcher = Scripted::with(vec![Ok(Hop::ok(&[big.as_slice()])), Ok(Hop::ok(&[b"two"]))]);
    let relay = UpdaterRelay::with_parts(fetcher, Arc::clone(&limiter));
    let download = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 51, &[], None);
    assert_eq!(
        relay.on_forward_frame(&download, Instant::now()),
        Disposition::Started
    );
    // The loop writes the first of the two frames; the last one stays queued.
    let started = Instant::now();
    let first = loop {
        if let Some(frame) = relay.poll_outbound() {
            break frame;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "no first frame");
        thread::sleep(Duration::from_millis(1));
    };
    let (first_id, _, first_payload, first_last, _) = decode_reply(&first);
    assert_eq!(
        (first_id, first_payload.len(), first_last),
        (51, 32_768, Some(false))
    );
    // The request is over once its last frame is queued. The test does not
    // wait for the permit: whoever sees the request over must find the right
    // to run already free, because the permit is given back before that is
    // said (and before the audit line is written).
    let started = Instant::now();
    while relay.status().in_flight {
        assert!(started.elapsed() < Duration::from_secs(5), "never over");
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !limiter.load(std::sync::atomic::Ordering::Acquire),
        "a request that is over still holds the right to run"
    );
    assert!(
        relay.is_active(),
        "but a frame of it is still to be written"
    );

    assert_eq!(
        relay.on_forward_frame(&check_frame(52), Instant::now()),
        Disposition::Started,
        "the unit's next request is carried, not answered with BUSY"
    );
    // Let the new request finish and queue its frame before anything is polled:
    // both the old tail and the new reply are then waiting, and the order they
    // come out in is the order the relay chose, not the order they were made.
    let started = Instant::now();
    while relay.status().requests_completed < 2 {
        assert!(started.elapsed() < Duration::from_secs(5), "never finished");
        thread::sleep(Duration::from_millis(1));
    }
    // The old reply's last frame still goes out first, then the new reply.
    let rest = drain(&relay);
    assert_eq!(rest.len(), 2);
    let (old_id, _, old_payload, old_last, old_error) = decode_reply(&rest[0]);
    assert_eq!(
        (old_id, old_payload.len(), old_last, old_error),
        (51, 7_232, Some(true), None)
    );
    let (new_id, _, new_payload, new_last, new_error) = decode_reply(&rest[1]);
    assert_eq!(
        (new_id, new_payload, new_last, new_error),
        (52, b"two".to_vec(), Some(true), None)
    );
    let status = relay.status();
    assert_eq!(status.requests_refused, 0, "nothing was turned away");
    assert_eq!(status.requests_completed, 2);
    assert!(!relay.is_active());
}

#[test]
fn a_request_blocked_in_the_middle_of_its_reply_is_still_running() {
    // The reply queue holds four frames. A 7-chunk body fills it and the
    // request's thread waits to queue its fifth frame, which is not the last:
    // the network work is not over, so a second request is answered BUSY until
    // the loop has taken enough frames.
    let (fetcher, senders) = Held::with_bodies(1);
    for _ in 0..6 {
        senders[0].send(vec![9; 40_000]).unwrap();
    }
    drop(senders);
    let relay = relay_with(fetcher);
    let download = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 71, &[], None);
    assert_eq!(
        relay.on_forward_frame(&download, Instant::now()),
        Disposition::Started
    );
    // Let the thread fill the queue and block on the fifth frame.
    thread::sleep(Duration::from_millis(300));
    assert!(relay.status().in_flight);
    assert_eq!(
        relay.on_forward_frame(&check_frame(72), Instant::now()),
        Disposition::Busy
    );
    // The loop takes frames; the request finishes; the next one is carried.
    let frames = drain(&relay);
    let download_frames: Vec<_> = frames
        .iter()
        .map(decode_reply)
        .filter(|reply| reply.0 == 71)
        .collect();
    assert_eq!(download_frames.len(), 8);
    assert_eq!(download_frames.last().unwrap().3, Some(true));
    let started = Instant::now();
    loop {
        if relay.on_forward_frame(&check_frame(73), Instant::now()) == Disposition::Started {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "never carried");
        thread::sleep(Duration::from_millis(2));
        drain(&relay);
    }
    drain(&relay);
}

#[test]
fn a_request_blocked_only_on_queueing_its_last_frame_does_not_turn_the_next_one_away() {
    // Four full chunks and a 1,000-byte remainder make five frames. The queue
    // holds four, so the thread blocks on the fifth, which is the LAST: the
    // network work is over and only the queueing is left. The request is not
    // done (its last frame is not queued), but it no longer holds the right to
    // run, so the unit's next request is carried, behind the old frames.
    let (fetcher, senders) = Held::with_bodies(1);
    for _ in 0..3 {
        senders[0].send(vec![5; 44_024]).unwrap();
    }
    drop(senders);
    let relay = relay_with(fetcher);
    let download = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 81, &[], None);
    assert_eq!(
        relay.on_forward_frame(&download, Instant::now()),
        Disposition::Started
    );
    thread::sleep(Duration::from_millis(300));
    assert!(
        relay.status().in_flight,
        "the last frame is not queued yet, so the request is not done"
    );
    assert_eq!(
        relay.on_forward_frame(&check_frame(82), Instant::now()),
        Disposition::Started,
        "but the network work is over, so the next request is not turned away"
    );
    let frames = drain(&relay);
    let order: Vec<(u64, Option<bool>)> = frames
        .iter()
        .map(|frame| {
            let reply = decode_reply(frame);
            (reply.0, reply.3)
        })
        .collect();
    assert_eq!(
        order,
        [
            (81, Some(false)),
            (81, Some(false)),
            (81, Some(false)),
            (81, Some(false)),
            (81, Some(true)),
            (82, Some(true))
        ]
    );
    assert_eq!(decode_reply(&frames[4]).2.len(), 1_000);
}

#[test]
fn two_sessions_in_one_process_never_run_two_requests_at_once() {
    let limiter = Arc::new(AtomicBool::new(false));
    let (held, senders) = Held::with_bodies(1);
    let first = UpdaterRelay::with_parts(held, Arc::clone(&limiter));
    let second = UpdaterRelay::with_parts(Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]), limiter);
    assert_eq!(
        first.on_forward_frame(&check_frame(1), Instant::now()),
        Disposition::Started
    );
    assert_eq!(
        second.on_forward_frame(&check_frame(1), Instant::now()),
        Disposition::Busy
    );
    drop(senders);
    drain(&first);
    // Once the first has finished the right to run is free again.
    let started = Instant::now();
    loop {
        if second.on_forward_frame(&check_frame(2), Instant::now()) == Disposition::Started {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the limiter was never released"
        );
        thread::sleep(Duration::from_millis(2));
    }
    drain(&second);
}

#[test]
fn switching_the_relay_off_ends_a_running_transfer() {
    let (fetcher, senders) = Held::with_bodies(1);
    let relay = relay_with(fetcher);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 1, &[], None);
    assert_eq!(
        relay.on_forward_frame(&frame, Instant::now()),
        Disposition::Started
    );
    assert!(relay.is_active());
    relay.use_fixed_settings(disabled());
    relay.maintain(Instant::now());
    assert!(!relay.is_active());
    let events = relay.take_events().join("\n");
    assert!(events.contains("switched off"), "{events}");
    drop(senders);
}

#[test]
fn a_unit_that_reports_its_updater_failed_ends_the_transfer() {
    let (fetcher, senders) = Held::with_bodies(1);
    let relay = relay_with(fetcher);
    relay.on_forward_frame(&check_frame(1), Instant::now());
    assert!(relay.is_active());
    let in_progress = pa::UpdaterMessage {
        action: pa::message_action::Enum::Update as i32,
        state: Some(pa::updater_message::State::State(
            pa::updater_state::Enum::RequestingData as i32,
        )),
        ..Default::default()
    };
    relay.on_updater_state(&in_progress.encode_to_vec());
    assert!(
        relay.is_active(),
        "an ordinary state change leaves the request alone"
    );
    let failed = pa::UpdaterMessage {
        action: pa::message_action::Enum::Update as i32,
        state: Some(pa::updater_message::State::State(
            pa::updater_state::Enum::Failed as i32,
        )),
        ..Default::default()
    };
    relay.on_updater_state(&failed.encode_to_vec());
    assert!(!relay.is_active());
    drop(senders);
}

#[test]
fn a_lost_session_cancels_the_running_request() {
    let (fetcher, senders) = Held::with_bodies(1);
    let relay = relay_with(fetcher);
    relay.on_forward_frame(&check_frame(1), Instant::now());
    relay.cancel_if_active("no device session");
    assert!(!relay.is_active());
    relay.cancel_if_active("no device session");
    drop(senders);
}

#[test]
fn a_cancelled_transfer_leaves_nothing_for_the_next_session_to_write() {
    let (fetcher, senders) = Held::with_bodies(1);
    for _ in 0..8 {
        senders[0].send(vec![7; 40_000]).unwrap();
    }
    let relay = relay_with(fetcher);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 31, &[], None);
    assert_eq!(
        relay.on_forward_frame(&frame, Instant::now()),
        Disposition::Started
    );
    let base = Instant::now();
    let mut old_session = RelayPump::new();
    let mut old_sink = Recording {
        sent: Vec::new(),
        clock: Rc::new(Cell::new(Duration::ZERO)),
        write_cost: Duration::ZERO,
        frames: Vec::new(),
    };
    // The old session writes its first chunk, with more queued behind it.
    let wall = Instant::now();
    loop {
        if old_session.tick(&relay, base, &mut old_sink).frames == 1 {
            break;
        }
        assert!(wall.elapsed() < Duration::from_secs(5), "no chunk arrived");
        thread::sleep(Duration::from_millis(1));
    }
    // The session is restarted or closed, and a new one starts.
    relay.cancel("the device session was restarted");
    assert!(!relay.is_active());
    let mut new_session = RelayPump::new();
    let mut new_sink = Recording {
        sent: Vec::new(),
        clock: Rc::new(Cell::new(Duration::ZERO)),
        write_cost: Duration::ZERO,
        frames: Vec::new(),
    };
    for step in 0..100 {
        new_session.note_read(true);
        let report = new_session.tick(&relay, base + Duration::from_millis(step), &mut new_sink);
        assert_eq!((report.keepalives, report.frames), (0, 0));
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        new_sink.sent.is_empty(),
        "no keepalive and no chunk of the old request reaches the new session"
    );
    assert!(!new_session.is_running());
    drop(senders);
}

#[test]
fn undecodable_frames_are_counted_and_never_answered() {
    let relay = relay_with(Scripted::with(vec![]));
    assert_eq!(
        relay.on_forward_frame(&[0xff, 0xff, 0xff], Instant::now()),
        Disposition::Undecodable
    );
    assert!(relay.poll_outbound().is_none());
    let status = relay.status();
    assert_eq!(status.requests_seen, 1);
    assert_eq!(status.requests_failed, 1);
}

#[test]
fn a_refused_domain_shows_up_in_the_status_by_host_name_only() {
    let relay = relay_with(Scripted::with(vec![]));
    let frame = request_frame(
        DOWNLOAD_UPDATE,
        &format!("https://downloads.example.net/image.bin?sig={SECRET_QUERY}"),
        3,
        &device_headers(),
        None,
    );
    relay.on_forward_frame(&frame, Instant::now());
    let frames = drain(&relay);
    assert_eq!(decode_reply(&frames[0]).4, Some(error_code::REFUSED));
    let status = relay.status();
    assert_eq!(
        status.last_refused_host.as_deref(),
        Some("downloads.example.net")
    );
    assert_eq!(status.requests_refused, 1);
    assert_eq!(status.refused_domain_requests, 1);
    assert!(status.last_refused_at_unix_ms.is_some());
    assert_eq!(status.recent[0].outcome, "refused:domain-not-allowed");
    assert_eq!(status.recent[0].host, "downloads.example.net");
    assert_eq!(status.recent[0].path_shape, "");
    let events = relay.take_events().join("\n");
    assert!(
        events.contains("REFUSED a request for domain downloads.example.net"),
        "{events}"
    );
    assert!(
        !events.contains("image.bin") && !events.contains(SECRET_QUERY),
        "{events}"
    );
}

#[test]
fn no_secret_in_a_header_a_path_or_a_query_reaches_a_log_a_status_or_an_error() {
    // Four requests, then a fifth while the relay is off. The secret is in the
    // header set, in the PATH and in the query, and one request really
    // redirects: to an allowed host (whose own path and query carry the
    // secrets and whose second hop then fails) and, in another request, to a
    // host that is not allowed. Every surface is checked after all of them.
    let fetcher = Scripted::with(vec![
        // 1: succeeds.
        Ok(Hop::ok(&[b"ok"])),
        // 2: redirects off the allow-list; refused before a second request.
        Ok(Hop::redirect(&format!(
            "https://downloads.example.net/dl/{SECRET_PATH}?sig={SECRET_QUERY}"
        ))),
        // 3: redirects to an allowed host, whose request then fails.
        Ok(Hop::redirect(&format!(
            "https://cdn.neuraldsp.com/img/{SECRET_PATH}/x.bin?sig={SECRET_QUERY}"
        ))),
        Err(FetchFailure::Tls),
    ]);
    let relay = relay_with(fetcher.clone());
    let query = format!("?sig={SECRET_QUERY}");
    let mut surfaces = Vec::new();
    // The reply surface below is readable text: a body shows up in it as itself.
    let probe = forward_response(None, 1, SECRET_QUERY.as_bytes().to_vec(), true, None);
    assert!(String::from_utf8_lossy(&probe.payload).contains(SECRET_QUERY));
    for (id, target) in [
        (
            1_u64,
            format!("https://cloud-api.neuraldsp.com/firmware/{SECRET_PATH}/latest{query}"),
        ),
        (
            2,
            format!("https://cloud-api.neuraldsp.com/firmware/{SECRET_PATH}/b{query}"),
        ),
        (
            3,
            format!("https://cloud-api.neuraldsp.com/firmware/{SECRET_PATH}/c{query}"),
        ),
        (
            4,
            format!("https://downloads.example.net/{SECRET_PATH}/x{query}"),
        ),
    ] {
        let frame = request_frame(CHECK, &target, id, &device_headers(), None);
        surfaces.push(format!("{:?}", decode_request(&frame).unwrap()));
        relay.on_forward_frame(&frame, Instant::now());
        // What the unit is sent back, errors included: the raw payload bytes
        // read as text, since `Debug` would print them as numbers and a text
        // secret could never match.
        for reply in drain(&relay) {
            surfaces.push(String::from_utf8_lossy(&reply.payload).into_owned());
            let (_, _, body, _, error) = decode_reply(&reply);
            surfaces.push(String::from_utf8_lossy(&body).into_owned());
            surfaces.push(format!("{error:?}"));
        }
    }
    // The redirect was really followed where it was allowed, and only there:
    // 1 + 1 (refused before a second request) + 2 + 0 requests.
    assert_eq!(fetcher.calls(), 4);
    relay.use_fixed_settings(disabled());
    relay.on_forward_frame(
        &request_frame(
            CHECK,
            &format!("https://cloud-api.neuraldsp.com/{SECRET_PATH}{query}"),
            5,
            &device_headers(),
            None,
        ),
        Instant::now(),
    );
    let status = relay.status();
    assert_eq!(status.recent.len(), 4);
    let outcomes: Vec<&str> = status
        .recent
        .iter()
        .map(|entry| entry.outcome.as_str())
        .collect();
    assert_eq!(
        outcomes,
        [
            "completed",
            "refused:domain-not-allowed",
            "network:tls",
            "refused:domain-not-allowed"
        ]
    );
    assert_eq!(
        status.recent[2].hops, 2,
        "the allowed redirect was followed"
    );
    assert_eq!(status.recent[2].host, "cdn.neuraldsp.com");
    assert!(
        status.recent[2].path_shape.contains("<opaque>"),
        "{}",
        status.recent[2].path_shape
    );
    surfaces.push(format!("{status:?}"));
    surfaces.push(serde_json::to_string(&status).unwrap());
    for entry in &status.recent {
        surfaces.push(entry.to_string());
    }
    surfaces.push(relay.take_events().join("\n"));
    for (index, surface) in surfaces.iter().enumerate() {
        for (label, secret) in [
            ("token", SECRET_TOKEN),
            ("serial", SECRET_SERIAL),
            ("query secret", SECRET_QUERY),
            ("path secret", SECRET_PATH),
        ] {
            // The message names the surface and the secret's kind, never the
            // values, so a failing run does not write them to the test log.
            assert!(
                !surface.contains(secret),
                "the {label} appears in output surface {index} at byte {:?}",
                surface.find(secret)
            );
        }
    }
}

#[test]
fn the_audit_record_holds_only_the_fields_it_promises() {
    let relay = relay_with(Scripted::with(vec![Ok(Hop::ok(&[b"abc"]))]));
    relay.on_forward_frame(&check_frame(1), Instant::now());
    drain(&relay);
    let entry = relay.status().recent.remove(0);
    let json = serde_json::to_value(&entry).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "atUnixMs",
            "bytesFromServer",
            "bytesToDevice",
            "hops",
            "host",
            "outcome",
            "pathShape",
            "request",
            "status"
        ]
    );
    assert_eq!(
        entry.to_string(),
        "updater-relay request=check host=cloud-api.neuraldsp.com path=/firmware/v1/latest \
         outcome=completed status=200 in=3 out=3 hops=1"
    );
}

// ---- the pump and its keepalive clock ---------------------------------------

#[test]
fn the_fixed_clock_fires_on_its_interval_and_never_in_a_burst() {
    let base = Instant::now();
    let second = Duration::from_secs(1);
    let mut clock = FixedClock::new(second);
    assert!(!clock.due(base) && !clock.is_running());
    clock.start(base);
    assert!(clock.due(base), "the first firing is due at once");
    clock.fired(base);
    assert!(!clock.due(base + Duration::from_millis(999)));
    assert!(clock.due(base + second));
    // Fired late, the next one is still on the original schedule.
    clock.fired(base + Duration::from_millis(1400));
    assert!(clock.due(base + Duration::from_secs(2)));
    // After a long stall it fires once and restarts from there.
    clock.fired(base + Duration::from_secs(30));
    assert!(!clock.due(base + Duration::from_secs(30)));
    assert!(clock.due(base + Duration::from_secs(31)));
    clock.stop();
    assert!(!clock.due(base + Duration::from_secs(60)) && !clock.is_running());
}

struct Recording {
    sent: Vec<(u16, Duration)>,
    clock: Rc<Cell<Duration>>,
    /// How long each type-61 write occupies the device lane in the simulation.
    write_cost: Duration,
    frames: Vec<OutboundMessage>,
}

impl RelaySink for Recording {
    fn send(&mut self, message: OutboundMessage) -> Result<(), String> {
        self.sent.push((message.message_type, self.clock.get()));
        if message.message_type == 61 {
            self.clock.set(self.clock.get() + self.write_cost);
            self.frames.push(message);
        }
        Ok(())
    }
}

#[test]
fn the_keepalive_runs_on_its_own_clock_through_a_long_transfer() {
    // 2,000,000 bytes become 62 reply chunks. Each one occupies the device lane
    // for 600 ms, so the transfer lasts about 37 s of simulated time.
    let (fetcher, senders) = Held::with_bodies(1);
    for _ in 0..50 {
        senders[0].send(vec![0x5a; 40_000]).unwrap();
    }
    drop(senders);
    let relay = relay_with(fetcher);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 11, &[], None);
    assert_eq!(
        relay.on_forward_frame(&frame, Instant::now()),
        Disposition::Started
    );

    let base = Instant::now();
    let clock = Rc::new(Cell::new(Duration::ZERO));
    let mut sink = Recording {
        sent: Vec::new(),
        clock: Rc::clone(&clock),
        write_cost: Duration::from_millis(600),
        frames: Vec::new(),
    };
    let mut pump = RelayPump::new();
    let wall = Instant::now();
    loop {
        // The loop makes one inbound read per pass, as the worker does; there is
        // no inbound traffic here, so every read finds the queue empty.
        pump.note_read(true);
        let report = pump.tick(&relay, base + clock.get(), &mut sink);
        assert_eq!(report.write_error, None);
        clock.set(clock.get() + Duration::from_millis(5));
        if report.frames == 0 && report.keepalives == 0 && !pump.awaiting_reads() {
            if !relay.is_active() {
                break;
            }
            // Waiting on the producing thread, not on the simulated lane.
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            wall.elapsed() < Duration::from_secs(30),
            "the transfer never finished"
        );
    }

    // The relay only ever sends the two types it is allowed to.
    assert!(sink.sent.iter().all(|(kind, _)| *kind == 32 || *kind == 61));
    let keepalives: Vec<Duration> = sink
        .sent
        .iter()
        .filter(|(kind, _)| *kind == 32)
        .map(|(_, at)| *at)
        .collect();
    let total = sink.sent.last().unwrap().1;
    assert!(
        total > Duration::from_secs(30),
        "the simulation was not long: {total:?}"
    );
    assert_eq!(
        keepalives[0],
        Duration::ZERO,
        "the clock starts with the transfer"
    );
    for pair in keepalives.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap <= Duration::from_millis(1700),
            "keepalive gap {gap:?} exceeds one interval plus one chunk write"
        );
    }
    assert!(
        keepalives.len() as u64 >= total.as_millis() as u64 / 1700,
        "{} keepalives in {total:?}",
        keepalives.len()
    );
    // And the transfer itself came through whole.
    assert_eq!(sink.frames.len(), 62);
    let mut received = 0;
    for (index, frame) in sink.frames.iter().enumerate() {
        let (updater_id, _, payload, last, error) = decode_reply(frame);
        assert_eq!(updater_id, 11);
        assert_eq!(error, None);
        assert_eq!(last, Some(index == 61));
        received += payload.len();
    }
    assert_eq!(received, 2_000_000);
    // When it is over the clock stops and the idle keepalive takes back over.
    let report = pump.tick(&relay, base + clock.get(), &mut sink);
    assert_eq!(report, Default::default());
    assert!(!pump.is_running());
}

/// The HID writer behind the pump, in simulated time: one frame being written
/// and one waiting in the single queue slot. `send` returns once the frame is
/// accepted into the slot, which is when the writer starts the frame ahead of
/// it, so a sender that is a frame ahead of the writer is held up for about one
/// frame's write time, as the real queue holds it.
struct Lane {
    clock: Rc<Cell<Duration>>,
    /// How long the writer needs for a type-61 frame.
    write_cost: Duration,
    last_start: Duration,
    last_done: Duration,
    /// When each frame started on the wire.
    wire: Vec<(u16, Duration)>,
    frames: Vec<OutboundMessage>,
}

impl Lane {
    fn new(clock: Rc<Cell<Duration>>, write_cost: Duration) -> Self {
        Self {
            clock,
            write_cost,
            last_start: Duration::ZERO,
            last_done: Duration::ZERO,
            wire: Vec::new(),
            frames: Vec::new(),
        }
    }
}

impl RelaySink for Lane {
    fn send(&mut self, message: OutboundMessage) -> Result<(), String> {
        let accepted = self.clock.get().max(self.last_start);
        let start = self.last_done.max(accepted);
        let cost = if message.message_type == 61 {
            self.write_cost
        } else {
            Duration::ZERO
        };
        self.last_start = start;
        self.last_done = start + cost;
        self.wire.push((message.message_type, start));
        if message.message_type == 61 {
            self.frames.push(message);
        }
        self.clock.set(accepted);
        Ok(())
    }
}

#[test]
fn inbound_is_read_while_a_reply_streams_and_a_failed_report_ends_it_promptly() {
    // The device loop reads one inbound event per pass. The unit sends 50
    // events a second here (more than the idle markers, tempo state and
    // progress reports of a real session) and reports its updater FAILED at
    // 20.010 s, in the middle of a transfer whose frames each take 156 ms to
    // write. Before the read quota the loop read about six events a second
    // for the whole transfer, so this backlog grew without bound and the
    // FAILED was handled minutes late.
    let (fetcher, senders) = Held::with_bodies(1);
    for _ in 0..200 {
        senders[0].send(vec![0x5a; 40_000]).unwrap();
    }
    let relay = relay_with(fetcher);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 21, &[], None);
    assert_eq!(
        relay.on_forward_frame(&frame, Instant::now()),
        Disposition::Started
    );
    let mut arrivals: Vec<(Duration, bool)> = (0..3_000_u64)
        .map(|n| (Duration::from_millis(20 * n), false))
        .collect();
    arrivals.push((Duration::from_millis(20_010), true));
    arrivals.sort();
    let failed_payload = pa::UpdaterMessage {
        action: pa::message_action::Enum::Update as i32,
        state: Some(pa::updater_message::State::State(
            pa::updater_state::Enum::Failed as i32,
        )),
        ..Default::default()
    }
    .encode_to_vec();

    let base = Instant::now();
    let clock = Rc::new(Cell::new(Duration::ZERO));
    let mut lane = Lane::new(Rc::clone(&clock), Duration::from_millis(156));
    let mut pump = RelayPump::new();
    let mut next = 0_usize;
    let mut worst_lag = Duration::ZERO;
    let mut failed_lag: Option<Duration> = None;
    let mut frames_when_failed = 0_usize;
    let wall = Instant::now();
    while failed_lag.is_none() || relay.is_active() {
        // One inbound read per pass: an event if one has arrived, otherwise
        // the 5 ms wait the real read makes, which finds the queue empty.
        let now = clock.get();
        let mut queue_was_empty = true;
        if next < arrivals.len() && arrivals[next].0 <= now {
            queue_was_empty = false;
            let (arrived, is_failed) = arrivals[next];
            next += 1;
            let lag = now - arrived;
            worst_lag = worst_lag.max(lag);
            if is_failed {
                relay.on_updater_state(&failed_payload);
                failed_lag = Some(lag);
                frames_when_failed = lane.frames.len();
            }
        } else {
            clock.set(now + Duration::from_millis(5));
        }
        pump.note_read(queue_was_empty);
        let report = pump.tick(&relay, base + clock.get(), &mut lane);
        assert_eq!(report.write_error, None);
        if report.frames == 0 && report.keepalives == 0 && !pump.awaiting_reads() {
            // Waiting on the producing thread, not on the simulated lane.
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            wall.elapsed() < Duration::from_secs(60),
            "the simulated transfer never ended"
        );
    }

    let failed_lag = failed_lag.expect("the unit's FAILED was read");
    assert!(
        failed_lag <= Duration::from_millis(400),
        "the FAILED report waited {failed_lag:?} behind inbound traffic"
    );
    assert!(
        worst_lag <= Duration::from_millis(400),
        "an inbound event waited {worst_lag:?} while a reply streamed"
    );
    assert!(
        frames_when_failed >= 100,
        "the transfer was not long: {frames_when_failed} frames"
    );
    // The FAILED ended the transfer: not one more reply frame is written.
    for _ in 0..60 {
        pump.note_read(true);
        let report = pump.tick(&relay, base + clock.get(), &mut lane);
        assert_eq!(report.frames, 0);
        clock.set(clock.get() + Duration::from_millis(5));
    }
    assert_eq!(lane.frames.len(), frames_when_failed);
    assert!(!pump.is_running());
    // The keepalive ran on its own clock throughout.
    let keepalives: Vec<Duration> = lane
        .wire
        .iter()
        .filter(|(kind, _)| *kind == 32)
        .map(|(_, at)| *at)
        .collect();
    assert!(keepalives.len() >= 18, "{} keepalives", keepalives.len());
    for pair in keepalives.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap <= Duration::from_millis(1600),
            "keepalive gap {gap:?} while inbound was being read"
        );
    }
    drop(senders);
}

#[test]
fn a_reply_frame_waits_for_its_reads_and_the_quota_never_holds_the_keepalive() {
    let (fetcher, senders) = Held::with_bodies(1);
    senders[0].send(vec![1; 40_000]).unwrap();
    senders[0].send(vec![2; 40_000]).unwrap();
    let relay = relay_with(fetcher);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 22, &[], None);
    relay.on_forward_frame(&frame, Instant::now());
    let mut sink = Recording {
        sent: Vec::new(),
        clock: Rc::new(Cell::new(Duration::ZERO)),
        write_cost: Duration::ZERO,
        frames: Vec::new(),
    };
    let mut pump = RelayPump::new();
    let base = Instant::now();
    // The producer thread queues its chunks in its own time: tick until the
    // first one is written. The keepalive goes out with the first pass.
    let wall = Instant::now();
    loop {
        let report = pump.tick(&relay, base, &mut sink);
        if report.frames == 1 {
            break;
        }
        assert!(wall.elapsed() < Duration::from_secs(5), "no chunk arrived");
        thread::sleep(Duration::from_millis(1));
    }
    let kinds: Vec<u16> = sink.sent.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(kinds, [32, 61]);
    assert!(pump.awaiting_reads());
    // No read, no second frame, however many passes go by.
    for step in 1..=10 {
        let report = pump.tick(&relay, base + Duration::from_millis(step), &mut sink);
        assert_eq!(report.frames, 0);
    }
    // The keepalive is due a second later and the quota does not hold it up.
    let late = pump.tick(&relay, base + Duration::from_millis(1_000), &mut sink);
    assert_eq!((late.keepalives, late.frames), (1, 0));
    // After the owed reads the next frame goes out.
    for _ in 0..READS_PER_FRAME {
        assert!(pump.awaiting_reads());
        pump.note_read(false);
    }
    assert!(!pump.awaiting_reads());
    let wall = Instant::now();
    loop {
        let report = pump.tick(&relay, base + Duration::from_millis(1_001), &mut sink);
        if report.frames == 1 {
            break;
        }
        assert!(wall.elapsed() < Duration::from_secs(5), "no second chunk");
        thread::sleep(Duration::from_millis(1));
    }
    assert!(pump.awaiting_reads());
    // A read that finds the queue empty ends the debt at once; a read that
    // returned a report (even one that completed only part of a frame) does not.
    pump.note_read(false);
    assert!(pump.awaiting_reads());
    pump.note_read(true);
    assert!(!pump.awaiting_reads());
    drop(senders);
}

#[test]
fn a_quiet_link_streams_at_the_write_rate_even_when_an_idle_poll_takes_15_ms() {
    // Windows can round the 5 ms receive poll up to its 15.6 ms timer tick.
    // With nothing inbound, every read finds the queue empty and must not
    // hold a frame back: before, 24 such reads cost 375 ms per 156 ms chunk,
    // so a reply streamed at 40 % of the write rate.
    let (fetcher, senders) = Held::with_bodies(1);
    for _ in 0..200 {
        senders[0].send(vec![0x33; 40_000]).unwrap();
    }
    let relay = relay_with(fetcher);
    let frame = request_frame(DOWNLOAD_UPDATE, CHECK_URL, 61, &[], None);
    assert_eq!(
        relay.on_forward_frame(&frame, Instant::now()),
        Disposition::Started
    );
    let base = Instant::now();
    let clock = Rc::new(Cell::new(Duration::ZERO));
    let mut lane = Lane::new(Rc::clone(&clock), Duration::from_millis(156));
    let mut pump = RelayPump::new();
    let idle_poll = Duration::from_micros(15_600);
    let span = Duration::from_secs(20);
    let wall = Instant::now();
    while clock.get() < span {
        // The poll waits its full time and finds nothing.
        clock.set(clock.get() + idle_poll);
        pump.note_read(true);
        let report = pump.tick(&relay, base + clock.get(), &mut lane);
        assert_eq!(report.write_error, None);
        if report.frames == 0 && report.keepalives == 0 {
            // Waiting on the producing thread, not on the simulated lane.
            thread::sleep(Duration::from_millis(1));
        }
        assert!(wall.elapsed() < Duration::from_secs(60), "never finished");
    }
    // The writer could write 20 s / 156 ms = 128 frames; the pump keeps up.
    let frames = lane.frames.len();
    assert!(
        frames >= 115,
        "only {frames} frames in 20 s of a quiet link"
    );
    relay.cancel("test over");
    drop(senders);
}

#[test]
fn an_idle_relay_sends_nothing_and_leaves_the_idle_keepalive_alone() {
    let relay = relay_with(Scripted::with(vec![]));
    let mut sink = Recording {
        sent: Vec::new(),
        clock: Rc::new(Cell::new(Duration::ZERO)),
        write_cost: Duration::ZERO,
        frames: Vec::new(),
    };
    let mut pump = RelayPump::new();
    for step in 0..100 {
        let report = pump.tick(
            &relay,
            Instant::now() + Duration::from_millis(step * 50),
            &mut sink,
        );
        assert_eq!(report, Default::default());
    }
    assert!(sink.sent.is_empty());
    assert!(!pump.is_running());
}

#[test]
fn a_failed_write_is_reported_and_not_retried() {
    struct Failing;
    impl RelaySink for Failing {
        fn send(&mut self, _message: OutboundMessage) -> Result<(), String> {
            Err("the HID writer stopped".into())
        }
    }
    let (fetcher, senders) = Held::with_bodies(1);
    let relay = relay_with(fetcher);
    relay.on_forward_frame(&check_frame(1), Instant::now());
    let mut pump = RelayPump::new();
    let report = pump.tick(&relay, Instant::now(), &mut Failing);
    assert_eq!(
        report.write_error.as_deref(),
        Some("the HID writer stopped")
    );
    assert_eq!((report.keepalives, report.frames), (0, 0));
    drop(senders);
}

#[test]
fn a_busy_refusal_is_sent_through_the_pump_as_a_type_61_frame() {
    let (fetcher, senders) = Held::with_bodies(1);
    let relay = relay_with(fetcher);
    relay.on_forward_frame(&check_frame(1), Instant::now());
    relay.on_forward_frame(&check_frame(2), Instant::now());
    let mut sink = Recording {
        sent: Vec::new(),
        clock: Rc::new(Cell::new(Duration::ZERO)),
        write_cost: Duration::ZERO,
        frames: Vec::new(),
    };
    let mut pump = RelayPump::new();
    pump.tick(&relay, Instant::now(), &mut sink);
    let kinds: Vec<u16> = sink.sent.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(kinds, [32, 61]);
    assert_eq!(decode_reply(&sink.frames[0]).4, Some(error_code::BUSY));
    drop(senders);
}

#[test]
fn the_relay_has_no_way_to_answer_any_other_message_family() {
    // The only frames the relay can build are type 61 replies; the pump adds
    // the type 32 keepalive. None of the encrypted families (18, 29, 30, 31,
    // 41, 58) is reachable from here.
    for kind in [CHECK, DOWNLOAD_UPDATE, 1, 2] {
        let frame = request_frame(kind, CHECK_URL, 1, &[], None);
        let relay = relay_with(Scripted::with(vec![Ok(Hop::ok(&[b"x"]))]));
        relay.on_forward_frame(&frame, Instant::now());
        for message in drain(&relay) {
            assert_eq!(message.message_type, 61);
        }
    }
}
