//! The host operating system's name and version, for a bug report.
//!
//! The webview can only say "Windows" (`userAgentData.platform`), which does
//! not tell a maintainer which Windows. `GetVersionExW` and `RtlGetVersion`
//! are not used: application-compatibility shims make both report an older
//! version to an app they target. The `CurrentVersion` registry key is what
//! Settings > System > About reads, and no shim rewrites it.

/// The registry values `describe_windows` formats, as read from
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
#[cfg(any(windows, test))]
#[derive(Debug, Default, PartialEq)]
pub struct WindowsVersionValues {
    /// `ProductName`, e.g. "Windows 10 Pro" (still "Windows 10" on Windows 11).
    pub product_name: Option<String>,
    /// `DisplayVersion` (e.g. "24H2"), or `ReleaseId` (e.g. "2009") on builds
    /// older than 20H2 that do not carry it.
    pub display_version: Option<String>,
    /// `CurrentBuild`, e.g. "26100".
    pub current_build: Option<String>,
    /// `UBR`, the update build revision, e.g. 2033.
    pub update_revision: Option<u32>,
}

/// The first build number of Windows 11.
///
/// Windows 11 kept `ProductName` as "Windows 10 …" for compatibility, so the
/// build number is the only registry value that tells the two apart.
#[cfg(any(windows, test))]
const FIRST_WINDOWS_11_BUILD: u32 = 22000;

/// A version a person reading the issue would recognize, such as
/// "Windows 11 Pro 24H2 (build 26100.2033)", or None when the registry named
/// neither the product nor the build.
#[cfg(any(windows, test))]
pub fn describe_windows(values: &WindowsVersionValues) -> Option<String> {
    let clean = |value: &Option<String>| {
        value
            .as_deref()
            .map(|text| {
                text.chars()
                    .filter(|character| !character.is_control())
                    .collect::<String>()
            })
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    let build = clean(&values.current_build);
    let build_number = build.as_deref().and_then(|text| text.parse::<u32>().ok());
    let product = clean(&values.product_name).map(|name| {
        match (build_number, name.strip_prefix("Windows 10")) {
            (Some(number), Some(edition)) if number >= FIRST_WINDOWS_11_BUILD => {
                format!("Windows 11{edition}")
            }
            _ => name,
        }
    });
    if product.is_none() && build.is_none() {
        return None;
    }
    let mut description = product.unwrap_or_else(|| "Windows".to_owned());
    if let Some(release) = clean(&values.display_version) {
        description.push(' ');
        description.push_str(&release);
    }
    if let Some(build) = build {
        match values.update_revision {
            Some(revision) => description.push_str(&format!(" (build {build}.{revision})")),
            None => description.push_str(&format!(" (build {build})")),
        }
    }
    Some(description)
}

#[cfg(windows)]
fn read_windows_version_values() -> Option<WindowsVersionValues> {
    let key = windows_registry::LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()?;
    Some(WindowsVersionValues {
        product_name: key.get_string("ProductName").ok(),
        display_version: key
            .get_string("DisplayVersion")
            .or_else(|_| key.get_string("ReleaseId"))
            .ok(),
        current_build: key.get_string("CurrentBuild").ok(),
        update_revision: key.get_u32("UBR").ok(),
    })
}

/// The host operating system, or None when it cannot be read.
#[cfg(windows)]
pub fn host_os_version() -> Option<String> {
    describe_windows(&read_windows_version_values()?)
}

/// This shell ships on Windows only; elsewhere the Feedback window falls back
/// to what the webview reports.
#[cfg(not(windows))]
pub fn host_os_version() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(
        product: &str,
        release: Option<&str>,
        build: &str,
        revision: Option<u32>,
    ) -> WindowsVersionValues {
        WindowsVersionValues {
            product_name: Some(product.to_owned()),
            display_version: release.map(str::to_owned),
            current_build: Some(build.to_owned()),
            update_revision: revision,
        }
    }

    #[test]
    fn windows_11_is_named_by_its_build_not_its_product_name() {
        assert_eq!(
            describe_windows(&values("Windows 10 Pro", Some("24H2"), "26100", Some(2033))),
            Some("Windows 11 Pro 24H2 (build 26100.2033)".to_owned())
        );
        assert_eq!(
            describe_windows(&values(
                "Windows 10 Enterprise",
                Some("21H2"),
                "22000",
                None
            )),
            Some("Windows 11 Enterprise 21H2 (build 22000)".to_owned())
        );
    }

    #[test]
    fn windows_10_keeps_its_product_name() {
        assert_eq!(
            describe_windows(&values(
                "Windows 10 Home",
                Some("22H2"),
                "19045",
                Some(4291)
            )),
            Some("Windows 10 Home 22H2 (build 19045.4291)".to_owned())
        );
    }

    #[test]
    fn missing_values_are_left_out_rather_than_invented() {
        assert_eq!(
            describe_windows(&WindowsVersionValues {
                current_build: Some("19045".to_owned()),
                ..WindowsVersionValues::default()
            }),
            Some("Windows (build 19045)".to_owned())
        );
        assert_eq!(
            describe_windows(&WindowsVersionValues {
                product_name: Some("Windows 10 Pro".to_owned()),
                ..WindowsVersionValues::default()
            }),
            Some("Windows 10 Pro".to_owned())
        );
        assert_eq!(describe_windows(&WindowsVersionValues::default()), None);
        assert_eq!(
            describe_windows(&values("  ", Some(""), " ", None)),
            None,
            "blank registry values name nothing"
        );
    }

    #[test]
    fn control_characters_in_a_registry_value_do_not_reach_the_report() {
        assert_eq!(
            describe_windows(&values(
                "Windows 10 Pro\n",
                Some("22H2\u{0}"),
                "19045",
                None
            )),
            Some("Windows 10 Pro 22H2 (build 19045)".to_owned())
        );
    }
}
