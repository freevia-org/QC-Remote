//! Opens external URLs in the user's default browser through a small
//! allowlist. Tauri 2's webview here has no opener plugin registered, so
//! `<a target="_blank">` anchors in the Help dialogs do nothing on their own;
//! every other outbound link in this app already routes through
//! `webbrowser::open` behind a Rust command (see `open_claude_desktop_download`
//! in `client_packages.rs`). This module generalizes that pattern instead of
//! opening an arbitrary string the front end hands over.

/// Hosts the Rust command is allowed to open. Kept intentionally small: every
/// destination the webview can reach has to be added here explicitly.
pub const ALLOWED_HOSTS: &[&str] = &["github.com", "freevia.org"];

const SCHEME: &str = "https://";
/// The longest URL the command opens. A prefilled bug report carries the
/// redacted diagnostics in its query, and the Feedback window keeps its links
/// at or under 6000 characters (`ISSUE_URL_MAX_LENGTH` in `about-content.ts`),
/// dropping the diagnostics rather than exceeding that. This ceiling sits
/// above it and below the ~8 KB request line GitHub's front end accepts; the
/// browser is started with the URL on its command line, whose own limit is
/// 32,767 characters.
pub const MAX_URL_LEN: usize = 8192;

/// Validates `url` against the allowlist before anything is handed to the OS
/// shell. Returns a short, user-safe reason on rejection.
pub fn validate_external_url(url: &str) -> Result<(), String> {
    if url != url.trim() {
        return Err("URL must not have leading or trailing whitespace".to_string());
    }
    if url.chars().any(|character| character.is_ascii_control()) {
        return Err("URL must not contain control characters".to_string());
    }
    if !url.starts_with(SCHEME) {
        return Err("URL must start with https://".to_string());
    }
    if url.len() > MAX_URL_LEN {
        return Err(format!("URL exceeds {MAX_URL_LEN} characters"));
    }

    let rest = &url[SCHEME.len()..];
    let authority = match rest.find(['/', '?', '#']) {
        Some(index) => &rest[..index],
        None => rest,
    };

    if authority.is_empty() {
        return Err("URL must include a host".to_string());
    }
    if authority.chars().any(|character| {
        character == '@' || character == ':' || character == '\\' || character.is_whitespace()
    }) {
        return Err("URL host must not contain credentials, a port, or whitespace".to_string());
    }
    if !ALLOWED_HOSTS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(authority))
    {
        return Err(format!("{authority} is not an allowed host"));
    }

    Ok(())
}

/// Validates `url`, then opens it in the user's default browser.
pub fn open_external_url(url: &str) -> Result<(), String> {
    validate_external_url(url)?;
    webbrowser::open(url)
        .map(|_| ())
        .map_err(|error| format!("Could not open {url}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_allowlisted_urls() {
        assert!(validate_external_url(
            "https://github.com/freevia-org/QC-Remote/issues/new?template=bug-report.yml&app-version=Windows%200.1.0"
        )
        .is_ok());
        assert!(validate_external_url("https://GitHub.com/freevia-org/QC-Remote").is_ok());
        assert!(validate_external_url("https://github.com").is_ok());
        assert!(validate_external_url("https://github.com/").is_ok());
        assert!(validate_external_url("https://freevia.org/qcremote/privacy").is_ok());
    }

    #[test]
    fn rejects_disallowed_urls() {
        let cases = [
            "http://github.com/freevia-org/QC-Remote",
            "https://example.com/",
            "https://github.com@localhost/x",
            "https://github.com:8443/",
            "https://github.com.example.com/",
            "https://freevia.org.example.com/",
            "javascript:alert(1)",
            " https://github.com",
            "https://",
        ];
        let too_long = format!("https://github.com/?q={}", "a".repeat(MAX_URL_LEN));
        assert!(validate_external_url(&too_long).is_err());
        let longest = format!(
            "https://github.com/?q={}",
            "a".repeat(MAX_URL_LEN - "https://github.com/?q=".len())
        );
        assert!(validate_external_url(&longest).is_ok());
        for case in cases {
            assert!(
                validate_external_url(case).is_err(),
                "expected {case:?} to be rejected"
            );
        }
    }
}
