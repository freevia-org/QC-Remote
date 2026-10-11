//! What the update relay will and will not fetch. Everything here is pure: no
//! sockets, no DNS, no clock. The device supplies the URL and the headers, so
//! both are untrusted input however the USB connection was made.

use reqwest::Url;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The domain the relay always allows, with every subdomain of it (a name is
/// in the domain when it is `neuraldsp.com` or ends in `.neuraldsp.com`; a name
/// that merely ends in the same letters, such as `evilneuraldsp.com`, is not).
/// Any other domain has to be added by name in the settings file
/// (`updaterRelayExtraDomains`); until then a request for it is refused and its
/// host name is recorded.
pub const NEURALDSP_DOMAIN: &str = "neuraldsp.com";

/// Most redirects one request may follow; every hop is validated again.
pub const MAX_REDIRECTS: u8 = 3;

/// Headers the device may supply: how many, and how many bytes in total.
const MAX_HEADERS: usize = 32;
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Why a request was not sent. The text names the rule, never a value the
/// device supplied other than a host name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Malformed,
    Scheme,
    UserInfo,
    Port,
    IpLiteral,
    /// The host is not in an allowed domain. Carries the host name only.
    Host(String),
}

impl Refusal {
    pub fn reason(&self) -> &'static str {
        match self {
            Refusal::Malformed => "url-malformed",
            Refusal::Scheme => "scheme-not-https",
            Refusal::UserInfo => "url-has-userinfo",
            Refusal::Port => "port-not-443",
            Refusal::IpLiteral => "host-is-ip-literal",
            Refusal::Host(_) => "domain-not-allowed",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Host(host) => write!(formatter, "{} ({host})", self.reason()),
            other => formatter.write_str(other.reason()),
        }
    }
}

/// The allowed domains and the URL rules around them.
#[derive(Clone, Debug)]
pub struct RelayPolicy {
    domains: Vec<String>,
}

impl RelayPolicy {
    /// `neuraldsp.com` plus the user's extra domains. Entries that are not
    /// plain domain names are dropped, so a bad settings value can only make
    /// the list shorter.
    pub fn new(extra_domains: &[String]) -> Self {
        let mut domains = vec![NEURALDSP_DOMAIN.to_owned()];
        for domain in extra_domains {
            if let Some(domain) = normalize_allowlist_domain(domain) {
                if !domains.contains(&domain) {
                    domains.push(domain);
                }
            }
        }
        Self { domains }
    }

    pub fn domains(&self) -> &[String] {
        &self.domains
    }

    /// Check one URL, for the first request and again for every redirect hop.
    /// Any path on an allowed domain is allowed. Address-level checks (private,
    /// loopback, link-local, CGNAT) happen where the name is resolved, against
    /// the addresses the connection will use.
    pub fn check_url(&self, url: &Url) -> Result<(), Refusal> {
        if url.scheme() != "https" {
            return Err(Refusal::Scheme);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Refusal::UserInfo);
        }
        // `Url` drops an explicit default port, so any remaining port is not 443.
        if url.port().is_some() {
            return Err(Refusal::Port);
        }
        let Some(host) = url.host_str() else {
            return Err(Refusal::Malformed);
        };
        // `Url` writes an IPv6 literal in brackets and rewrites every numeric
        // IPv4 spelling (`127.1`, `0x7f.1`, `2130706433`) to dotted decimal, so
        // these two tests see every address literal.
        if host.starts_with('[') || host.parse::<Ipv4Addr>().is_ok() {
            return Err(Refusal::IpLiteral);
        }
        let host = host.to_ascii_lowercase();
        if self.domains.iter().any(|domain| in_domain(&host, domain)) {
            Ok(())
        } else {
            Err(Refusal::Host(host))
        }
    }
}

/// Whether `host` is `domain` or a subdomain of it, on a label boundary.
fn in_domain(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// Lower-case a configured domain and accept it only if it is a plain DNS name:
/// two or more labels of letters, digits and inner hyphens, no wildcard, no
/// port, no path, and not an address.
pub fn normalize_allowlist_domain(host: &str) -> Option<String> {
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 || host.parse::<IpAddr>().is_ok() {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return None;
    }
    let plain = labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    });
    // A last label of digits only is an address in disguise (`1.2.3.4`, `127.1`).
    let numeric_tail = labels
        .last()
        .is_some_and(|label| label.bytes().all(|byte| byte.is_ascii_digit()));
    (plain && !numeric_tail).then_some(host)
}

/// Whether a resolved address may be connected to: only globally routable
/// unicast addresses. Everything on the user's own network and everything
/// reserved is refused, including the IPv4 addresses embedded in IPv4-mapped,
/// IPv4-translated, NAT64 and 6to4 IPv6 forms. The special-purpose blocks that
/// are listed are refused whole, the IPv4 anycast and relay ranges
/// (192.88.99.0/24, 192.31.196.0/24, 192.52.193.0/24, 192.175.48.0/24) among
/// them.
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_multicast()
        || address.is_documentation()
        || a == 0
        // 100.64.0.0/10 carrier-grade NAT.
        || (a == 100 && (64..=127).contains(&b))
        // 192.0.0.0/24 IETF protocol assignments.
        || (a == 192 && b == 0 && c == 0)
        // 198.18.0.0/15 benchmarking.
        || (a == 198 && (b == 18 || b == 19))
        // Special-purpose anycast and relay ranges that no update host lives
        // in: 192.88.99.0/24 (deprecated 6to4 relay anycast), 192.31.196.0/24
        // (AS112), 192.52.193.0/24 (AMT) and 192.175.48.0/24 (AS112 direct
        // delegation).
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 31 && c == 196)
        || (a == 192 && b == 52 && c == 193)
        || (a == 192 && b == 175 && c == 48)
        // 240.0.0.0/4 reserved.
        || a >= 240)
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    if address.is_unspecified() || address.is_loopback() || address.is_multicast() {
        return false;
    }
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_v4(mapped);
    }
    let segments = address.segments();
    // ::/96 (IPv4-compatible) and 64:ff9b::/96 (NAT64) carry an IPv4 address
    // in the last 32 bits; 2002::/16 (6to4) carries one after the prefix.
    let embedded = |high: u16, low: u16| {
        Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
    };
    if segments[..6] == [0_u16; 6] {
        return false;
    }
    if segments[..6] == [0x64_u16, 0xff9b, 0, 0, 0, 0] {
        return is_public_v4(embedded(segments[6], segments[7]));
    }
    // ::ffff:0:0:0/96 (IPv4-translated, RFC 2765, `::ffff:0:a.b.c.d`) also
    // carries an IPv4 address in the last 32 bits. It is not the IPv4-mapped
    // form `to_ipv4_mapped` knows (`::ffff:a.b.c.d`, whose sixth segment is
    // 0xffff), so it needs its own test.
    if segments[..4] == [0_u16; 4] && segments[4] == 0xffff && segments[5] == 0 {
        return is_public_v4(embedded(segments[6], segments[7]));
    }
    if segments[0] == 0x2002 {
        return is_public_v4(embedded(segments[1], segments[2]));
    }
    !((segments[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
        || (segments[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || (segments[0] & 0xffc0) == 0xfec0 // fec0::/10 site-local
        || (segments[0] == 0x2001 && segments[1] == 0x0db8) // documentation
        || (segments[0] == 0x2001 && (segments[1] & 0xfe00) == 0) // 2001::/23 IETF protocol assignments, Teredo included
        || segments[0] == 0x5f00 // 5f00::/16 SRv6 segment identifiers
        || (segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 0x0001) // 64:ff9b:1::/48 local-use NAT64
        || (segments[0] == 0x3fff && (segments[1] & 0xf000) == 0) // 3fff::/20 documentation
        || (segments[0] == 0x0100 && segments[1..4] == [0_u16; 3])) // discard-only
}

/// A header the device supplied. The value is never printed: `Debug` shows the
/// name and the value's length, because the set carries a bearer token, a
/// request signature and the unit's serial number.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceHeader {
    pub name: String,
    pub value: String,
}

impl fmt::Debug for DeviceHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: <redacted len={}>",
            self.name,
            self.value.len()
        )
    }
}

/// Why the device's header set was not forwarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderRefusal {
    TooMany,
    TooLarge,
    BadName,
    BadValue,
}

/// Headers a proxy must not pass on, plus the two the HTTP client sets itself.
const NOT_FORWARDED: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "expect",
];

/// The headers to send on the first hop: exactly what the device supplied,
/// minus the ones that belong to the connection rather than the request.
pub fn forwardable_headers(headers: &[DeviceHeader]) -> Result<Vec<DeviceHeader>, HeaderRefusal> {
    if headers.len() > MAX_HEADERS {
        return Err(HeaderRefusal::TooMany);
    }
    let total: usize = headers
        .iter()
        .map(|header| header.name.len() + header.value.len())
        .sum();
    if total > MAX_HEADER_BYTES {
        return Err(HeaderRefusal::TooLarge);
    }
    let mut kept = Vec::with_capacity(headers.len());
    for header in headers {
        let name_ok = !header.name.is_empty()
            && header
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));
        if !name_ok {
            return Err(HeaderRefusal::BadName);
        }
        if header
            .value
            .bytes()
            .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
        {
            return Err(HeaderRefusal::BadValue);
        }
        let lowered = header.name.to_ascii_lowercase();
        if NOT_FORWARDED.contains(&lowered.as_str()) {
            continue;
        }
        kept.push(header.clone());
    }
    Ok(kept)
}

/// What a redirect to a different host may carry. The credentials and the
/// unit's identity were addressed to the first host, so a hop elsewhere (a
/// download host named by a redirect) gets only the byte range.
pub fn cross_host_headers(headers: &[DeviceHeader]) -> Vec<DeviceHeader> {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("range"))
        .cloned()
        .collect()
}

/// The path with long or token-like segments replaced (segments of up to 24
/// bytes that are not mixed letters and digits of 16 bytes or more are kept as
/// they are), and the query reduced to its key names. Free of query values and
/// of long tokens, which is what makes it fit for a log; a short identifier in
/// a path would still appear in it.
pub fn path_shape(url: &Url) -> String {
    let mut shape = String::new();
    for segment in url.path().split('/').filter(|segment| !segment.is_empty()) {
        shape.push('/');
        shape.push_str(&shape_segment(segment));
    }
    if shape.is_empty() {
        shape.push('/');
    }
    let keys: Vec<String> = url
        .query_pairs()
        .map(|(key, _)| key.chars().take(24).collect())
        .collect();
    if !keys.is_empty() {
        shape.push('?');
        shape.push_str(&keys.join("&"));
    }
    shape.chars().take(160).collect()
}

fn shape_segment(segment: &str) -> String {
    let long = segment.len() > 24;
    let has_digit = segment.bytes().any(|byte| byte.is_ascii_digit());
    let has_alpha = segment.bytes().any(|byte| byte.is_ascii_alphabetic());
    let mixed_token = segment.len() >= 16 && has_digit && has_alpha;
    if long || mixed_token {
        "<opaque>".to_owned()
    } else {
        segment.to_owned()
    }
}
