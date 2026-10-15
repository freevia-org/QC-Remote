//! Legacy settings compatibility and device header representation.
use std::{fmt, net::IpAddr};
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
