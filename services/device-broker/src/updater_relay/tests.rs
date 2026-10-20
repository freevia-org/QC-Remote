use super::{
    executor::{self, ChunkSink, Failure, SinkClosed, USB_CHUNK_BYTES},
    fetch::{FetchFailure, FetchRequest, FetchedHop, HttpBody, HttpFetcher},
    request::decode_request,
    response, Disposition, UpdaterRelay,
};
use crate::tray_settings::UpdaterRelaySettings;
use prost::Message as _;
use quadcortex::{commands::OutboundMessage, proto::cortex_protobuf_v2 as pa};
use std::{
    collections::VecDeque,
    sync::{atomic::AtomicBool, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn frame(kind: i32, url: &str, id: u64, body: Option<&str>) -> Vec<u8> {
    pa::UpdaterForwardMessage {
        action: 1,
        updater_request_id: id,
        request: Some(pa::updater_forward_message::Request::Request(
            pa::UpdaterForwardRequest {
                r#type: kind,
                url: url.into(),
                timeout: 0,
                payload: body.map(|s| pa::updater_forward_request::Payload::Payload(s.into())),
                headers: vec![pa::ProductForwardHeaderPair {
                    name: "X-Device-Header".into(),
                    value: "exact".into(),
                }],
            },
        )),
        ..Default::default()
    }
    .encode_to_vec()
}

#[test]
fn demo_cached_rpc_relay_diagnostics_do_not_wait_or_invent_settings() {
    let relay = UpdaterRelay::with_parts(Fake::new(200, vec![]), Arc::new(AtomicBool::new(false)));
    assert!(relay.try_cached_status().unwrap().enabled);
    relay.use_fixed_settings(UpdaterRelaySettings {
        enabled: false,
        ..UpdaterRelaySettings::default()
    });
    assert!(!relay.try_cached_status().unwrap().enabled);
    {
        let _held = relay.state.lock().unwrap();
        assert!(relay.try_cached_status().is_none());
    }
    // Selecting a directory invalidates the cache. The try-only projection
    // must not load it or invent the default until normal maintenance reads it.
    relay.set_settings_directory(std::path::PathBuf::from("."));
    assert!(relay.try_cached_status().is_none());
    assert!(relay.settings.lock().unwrap().read_at.is_none());
}
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    url: String,
    headers: Vec<(String, String)>,
    payload: Vec<u8>,
    timeout: Duration,
}
struct Fake {
    seen: Mutex<Vec<Seen>>,
    pieces: Mutex<VecDeque<Result<Vec<u8>, FetchFailure>>>,
    status: u16,
    declared: Option<u64>,
}
impl Fake {
    fn new(status: u16, pieces: Vec<Result<Vec<u8>, FetchFailure>>) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            pieces: Mutex::new(pieces.into()),
            status,
            declared: Some(u64::MAX),
        })
    }
}
struct Pieces(VecDeque<Result<Vec<u8>, FetchFailure>>);
impl HttpBody for Pieces {
    fn next_chunk(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, FetchFailure> {
        self.0.pop_front().transpose()
    }
}
impl HttpFetcher for Fake {
    fn get(&self, r: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure> {
        self.seen.lock().unwrap().push(Seen {
            url: r.url.as_str().into(),
            headers: r
                .headers
                .iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect(),
            payload: r.payload.into(),
            timeout: r.head_timeout,
        });
        Ok(FetchedHop {
            status: self.status,
            headers: vec![],
            location: Some("https://must-not-follow.example/".into()),
            content_length: self.declared,
            body: Box::new(Pieces(std::mem::take(&mut *self.pieces.lock().unwrap()))),
        })
    }
}
#[derive(Default)]
struct Sink {
    frames: Vec<(Vec<u8>, bool, Option<i32>)>,
}
impl ChunkSink for Sink {
    fn emit(&mut self, p: Vec<u8>, last: bool, status: Option<i32>) -> Result<(), SinkClosed> {
        self.frames.push((p, last, status));
        Ok(())
    }
}
fn run(fake: &Fake, wire: &[u8]) -> (executor::Outcome, Sink) {
    let request = decode_request(wire).unwrap();
    let mut sink = Sink::default();
    let result = executor::execute(fake, &request, &Arc::new(AtomicBool::new(false)), &mut sink);
    (result, sink)
}
#[test]
fn every_type_preserves_destination_query_headers_and_request_body() {
    for kind in [0, 1, 3, 99] {
        for url in [
            "https://unknown.example:8443/a?signature=A%2Bb%2Fc%3D&order=1&order=2",
            "http://127.0.0.1:8080/x",
            "https://[::1]:444/x",
        ] {
            let fake = Fake::new(200, vec![Ok(b"response".to_vec())]);
            let (outcome, sink) = run(&fake, &frame(kind, url, 9, Some("{\"device\":true}")));
            assert_eq!(outcome.failure, None);
            assert_eq!(sink.frames, vec![(b"response".to_vec(), true, Some(200))]);
            assert_eq!(
                fake.seen.lock().unwrap()[0],
                Seen {
                    url: url.into(),
                    headers: vec![("X-Device-Header".into(), "exact".into())],
                    payload: b"{\"device\":true}".to_vec(),
                    timeout: Duration::ZERO
                }
            );
        }
    }
}
#[test]
fn response_bodies_have_no_small_body_cap() {
    for kind in [0, 1, 3] {
        let bytes = vec![0xa5; 5 * 1024 * 1024 + 17];
        let fake = Fake::new(206, vec![Ok(bytes.clone())]);
        let (outcome, sink) = run(&fake, &frame(kind, "https://cdn.example/image", 10, None));
        assert_eq!(outcome.failure, None);
        assert_eq!(outcome.bytes_from_server, bytes.len() as u64);
        assert_eq!(outcome.bytes_to_device, bytes.len() as u64);
        assert_eq!(
            sink.frames
                .iter()
                .flat_map(|f| f.0.iter().copied())
                .collect::<Vec<_>>(),
            bytes
        );
        assert!(sink.frames.iter().all(|f| f.0.len() <= USB_CHUNK_BYTES
            && (if f.1 { f.2 == Some(206) } else { f.2.is_none() })));
        assert!(sink.frames.last().unwrap().1);
        assert!(sink.frames[..sink.frames.len() - 1].iter().all(|f| !f.1));
    }
}
#[test]
fn all_http_statuses_and_bodies_pass_through_without_redirect_or_retry() {
    for status in [
        200, 204, 206, 301, 302, 307, 308, 400, 401, 403, 404, 500, 503,
    ] {
        let body = vec![0, 255, 0x1f, 0x8b, 13, 10];
        let fake = Fake::new(status, vec![Ok(body.clone())]);
        let (outcome, sink) = run(&fake, &frame(0, "https://any.example/original", 11, None));
        assert_eq!(fake.seen.lock().unwrap().len(), 1);
        assert_eq!(sink.frames, vec![(body, true, Some(i32::from(status)))]);
        assert_eq!(outcome.status, Some(status));
    }
}
#[test]
fn empty_response_still_emits_the_actual_status_and_final_flag() {
    let fake = Fake::new(204, vec![]);
    let (_, sink) = run(&fake, &frame(0, "https://any.example/", 12, None));
    assert_eq!(sink.frames, vec![(vec![], true, Some(204))]);
}
#[test]
fn a_transport_failure_is_not_reported_as_http_success() {
    let fake = Fake::new(200, vec![Err(FetchFailure::Tls)]);
    let (outcome, sink) = run(&fake, &frame(0, "https://any.example/", 13, None));
    assert_eq!(outcome.failure, Some(Failure::Network(FetchFailure::Tls)));
    assert_eq!(sink.frames[0].2, Some(503));
}
#[test]
fn request_decoding_has_no_size_or_header_count_policy() {
    let body = "z".repeat(100_000);
    let wire = frame(2, "http://localhost:80/", 14, Some(&body));
    let mut message = pa::UpdaterForwardMessage::decode(wire.as_slice()).unwrap();
    if let Some(pa::updater_forward_message::Request::Request(r)) = &mut message.request {
        r.headers = (0..100)
            .map(|i| pa::ProductForwardHeaderPair {
                name: format!("X-{i}"),
                value: "value".into(),
            })
            .collect();
    }
    let request = decode_request(&message.encode_to_vec()).unwrap();
    assert_eq!(request.payload, body.as_bytes());
    assert_eq!(request.headers.len(), 100);
}
#[test]
fn updater_reply_matches_cc_presence_status_and_correlation() {
    let reply = response::forward_response(None, 19, b"body".to_vec(), true, Some(200));
    let decoded = pa::UpdaterForwardMessage::decode(reply.payload.as_slice()).unwrap();
    assert!(matches!(
        decoded.request_id,
        Some(pa::updater_forward_message::RequestId::RequestId(0))
    ));
    assert_eq!(decoded.updater_request_id, 19);
    let Some(pa::updater_forward_message::Response::Response(r)) = decoded.response else {
        panic!("response missing")
    };
    assert_eq!(r.payload, b"body");
    assert!(matches!(
        r.error_code,
        Some(pa::updater_forward_response::ErrorCode::ErrorCode(200))
    ));
}
fn drain(relay: &UpdaterRelay) -> Vec<OutboundMessage> {
    let until = Instant::now() + Duration::from_secs(5);
    let mut frames = vec![];
    while relay.is_active() {
        if let Some(f) = relay.poll_outbound() {
            frames.push(f.into_parts().0)
        } else {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(1));
        }
    }
    frames
}
#[test]
fn requests_are_accepted_without_a_busy_domain_or_size_refusal() {
    let fake = Fake::new(200, vec![Ok(b"ok".to_vec())]);
    let relay = UpdaterRelay::with_parts(fake.clone(), Arc::new(AtomicBool::new(true)));
    relay.use_fixed_settings(UpdaterRelaySettings {
        extra_domains: vec!["restricted.example".into()],
        max_download_bytes: 1,
        ..Default::default()
    });
    for id in 0..12 {
        assert_eq!(
            relay.on_forward_frame(
                &frame(2, "http://127.0.0.1:9999/image", id, None),
                Instant::now()
            ),
            Disposition::Started
        );
    }
    let replies = drain(&relay);
    assert_eq!(replies.len(), 12);
    assert_eq!(fake.seen.lock().unwrap().len(), 12);
    assert_eq!(relay.status().requests_refused, 0);
}
#[test]
fn disconnect_discards_queued_replies_from_the_old_session() {
    let fake = Fake::new(200, vec![Ok(vec![1; 2 * USB_CHUNK_BYTES])]);
    let relay = UpdaterRelay::with_parts(fake, Arc::new(AtomicBool::new(false)));
    relay.on_forward_frame(&frame(3, "https://any.example/", 40, None), Instant::now());
    relay.cancel("session ended");
    assert!(!relay.is_active());
    assert!(relay.poll_outbound().is_none());
}
#[test]
fn explicitly_disabling_internet_forwarding_remains_effective() {
    let relay = UpdaterRelay::new();
    relay.use_fixed_settings(UpdaterRelaySettings {
        enabled: false,
        ..Default::default()
    });
    assert_eq!(
        relay.on_forward_frame(&frame(0, "https://any.example/", 1, None), Instant::now()),
        Disposition::Disabled
    );
    assert!(!relay.status().enabled);
}
#[test]
fn local_status_does_not_expose_http_secrets() {
    let status = serde_json::to_string(&UpdaterRelay::new().status()).unwrap();
    assert!(!status.contains("headers"));
    assert!(!status.contains("allowedDomains"));
}
#[test]
fn fixed_keepalive_clock_does_not_drift_or_burst() {
    use super::pump::FixedClock;
    let start = Instant::now();
    let mut clock = FixedClock::new(Duration::from_secs(1));
    clock.start(start);
    assert!(clock.due(start));
    clock.fired(start);
    assert!(!clock.due(start + Duration::from_millis(999)));
    assert!(clock.due(start + Duration::from_secs(1)));
    clock.fired(start + Duration::from_secs(5));
    assert!(!clock.due(start + Duration::from_secs(5)));
    assert!(clock.due(start + Duration::from_secs(6)));
}

#[test]
fn firmware_header_operation_forwards_all_headers_without_reading_image_bytes() {
    struct NoRead;
    impl HttpBody for NoRead {
        fn next_chunk(&mut self, _: Duration) -> Result<Option<Vec<u8>>, FetchFailure> {
            panic!("header operation must not consume body")
        }
    }
    struct Header;
    impl HttpFetcher for Header {
        fn get(&self, _: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure> {
            Ok(FetchedHop {
                status: 200,
                headers: vec![
                    ("content-length".into(), "154254140".into()),
                    ("server".into(), "upstream".into()),
                ],
                location: None,
                content_length: Some(154254140),
                body: Box::new(NoRead),
            })
        }
    }
    let request =
        decode_request(&frame(2, "https://arbitrary.cdn.example/image", 360, None)).unwrap();
    let mut sink = Sink::default();
    let outcome = executor::execute(
        &Header,
        &request,
        &Arc::new(AtomicBool::new(false)),
        &mut sink,
    );
    assert_eq!(outcome.failure, None);
    assert_eq!(outcome.bytes_from_server, 0);
    assert_eq!(
        sink.frames,
        vec![(
            br#"{"Content-Length":"154254140","Server":"upstream"}"#.to_vec(),
            true,
            Some(200)
        )]
    );
}
#[test]
fn intermediate_updater_chunks_omit_status_as_cortex_control_does() {
    let reply = response::forward_response(None, 355, vec![5; 32], false, None);
    let decoded = pa::UpdaterForwardMessage::decode(reply.payload.as_slice()).unwrap();
    let Some(pa::updater_forward_message::Response::Response(r)) = decoded.response else {
        panic!("response missing")
    };
    assert!(r.error_code.is_none());
    assert!(matches!(
        r.is_last_chunk,
        Some(pa::updater_forward_response::IsLastChunk::IsLastChunk(
            false
        ))
    ));
}

#[test]
fn header_metadata_preserves_unknown_duplicate_and_future_fields() {
    struct Empty;
    impl HttpBody for Empty {
        fn next_chunk(&mut self, _: Duration) -> Result<Option<Vec<u8>>, FetchFailure> {
            panic!("metadata must not read body")
        }
    }
    struct Headers;
    impl HttpFetcher for Headers {
        fn get(&self, _: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure> {
            Ok(FetchedHop {
                status: 206,
                headers: vec![
                    ("content-length".into(), "00042".into()),
                    ("x-future-response".into(), "first".into()),
                    ("x-future-response".into(), "second".into()),
                    ("set-cookie".into(), "a=1; HttpOnly".into()),
                ],
                location: None,
                content_length: Some(42),
                body: Box::new(Empty),
            })
        }
    }
    let request = decode_request(&frame(2, "http://127.0.0.1:8080/", 400, None)).unwrap();
    let mut sink = Sink::default();
    let outcome = executor::execute(
        &Headers,
        &request,
        &Arc::new(AtomicBool::new(false)),
        &mut sink,
    );
    assert_eq!(outcome.failure, None);
    assert_eq!(sink.frames[0].2, Some(206));
    assert_eq!(sink.frames[0].0,br#"{"Content-Length":"00042","X-Future-Response":"first","X-Future-Response":"second","Set-Cookie":"a=1; HttpOnly"}"#);
}

#[test]
fn queued_updater_frame_expiry_and_retirement_survive_http_task_completion() {
    let relay = UpdaterRelay::with_parts(Fake::new(200, vec![]), Arc::new(AtomicBool::new(false)));
    let cancel = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    sender
        .send(quadcortex_forwarder::ForwardedMessage {
            message: response::forward_response(Some(1), 2, vec![1], true, Some(200)),
            deadline: Some(Instant::now() - Duration::from_millis(1)),
            cancel: Arc::clone(&cancel),
        })
        .ok()
        .unwrap();
    drop(sender);
    relay.tasks.lock().unwrap().push(super::Task {
        cancel: Arc::clone(&cancel),
        replies: receiver,
        thread: thread::spawn(|| {}),
    });
    assert!(relay.poll_outbound().is_none());
    relay
        .response_cancellations
        .lock()
        .unwrap()
        .push(Arc::downgrade(&cancel));
    relay.cancel_if_active("setting disabled after HTTP completion");
    assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn an_update_counts_as_in_progress_between_its_requests_and_while_the_unit_says_so() {
    let relay = UpdaterRelay::new();
    relay.use_fixed_settings(UpdaterRelaySettings {
        enabled: false,
        ..Default::default()
    });
    let start = Instant::now();
    assert!(!relay.update_in_progress(start));
    // A request, even one not forwarded, starts the quiet period; the gap
    // before the next one of a download is still the update.
    relay.on_forward_frame(&frame(0, "https://neuraldsp.com/x", 1, None), start);
    assert!(!relay.is_active());
    assert!(relay.update_in_progress(start + super::UPDATE_QUIET_AFTER - Duration::from_secs(1)));
    assert!(!relay.update_in_progress(start + super::UPDATE_QUIET_AFTER));
    // The unit's own updater state keeps it in progress past the quiet period.
    let state = |value: pa::updater_state::Enum| {
        pa::UpdaterMessage {
            state: Some(pa::updater_message::State::State(value as i32)),
            ..Default::default()
        }
        .encode_to_vec()
    };
    relay.on_updater_state(&state(pa::updater_state::Enum::Updating));
    assert!(relay.update_in_progress(start + Duration::from_secs(600)));
    relay.on_updater_state(&state(pa::updater_state::Enum::Idle));
    assert!(!relay.update_in_progress(start + Duration::from_secs(600)));
}
