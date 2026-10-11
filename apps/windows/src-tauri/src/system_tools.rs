//! Absolute paths to the Windows tools this app launches.
//!
//! `Command::new("reg.exe")` resolves through the Windows search order, which
//! looks in the application directory and the current directory before the
//! system directories. A per-user install directory, or whatever directory the
//! app was started from, could then supply its own `reg.exe` or
//! `powershell.exe`, and it would run with the user's rights every time the app
//! read the registry. These helpers name the copies Windows itself ships.
use std::path::{Path, PathBuf};

// SAFETY: these declarations match the kernel32 signatures `UINT GetSystemDirectoryW(LPWSTR,
// UINT)` and `UINT GetSystemWindowsDirectoryW(LPWSTR, UINT)`. Callers pass a writable
// buffer and its length in UTF-16 units, as `known_directory` does.
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn GetSystemWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
}

// SAFETY: `read` is only ever one of the two functions declared above, and is called with a
// writable buffer and that buffer's length (see the SAFETY comment in the body).
#[cfg(windows)]
fn known_directory(read: unsafe extern "system" fn(*mut u16, u32) -> u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = vec![0_u16; 260];
    loop {
        // SAFETY: the buffer is writable for `buffer.len()` UTF-16 units and
        // the API writes at most that many, returning the length it needs
        // (including the terminator) when the buffer is too small.
        let length = unsafe { read(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        if length == 0 {
            return None;
        }
        if length < buffer.len() {
            buffer.truncate(length);
            let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer));
            return path.is_absolute().then_some(path);
        }
        buffer.resize(length, 0);
    }
}

/// `C:\Windows\System32`, from the OS rather than the environment.
#[cfg(any(windows, test))]
pub(crate) fn system32_directory() -> PathBuf {
    system_directory().unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"))
}

/// `C:\Windows`, from the OS rather than the environment.
#[cfg(any(windows, test))]
pub(crate) fn windows_directory() -> PathBuf {
    system_windows_directory().unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

#[cfg(windows)]
fn system_directory() -> Option<PathBuf> {
    known_directory(GetSystemDirectoryW)
}

#[cfg(windows)]
fn system_windows_directory() -> Option<PathBuf> {
    known_directory(GetSystemWindowsDirectoryW)
}

#[cfg(all(test, not(windows)))]
fn system_directory() -> Option<PathBuf> {
    None
}

#[cfg(all(test, not(windows)))]
fn system_windows_directory() -> Option<PathBuf> {
    None
}

#[cfg(any(windows, test))]
pub(crate) fn reg_exe() -> PathBuf {
    system32_directory().join("reg.exe")
}

#[cfg(any(windows, test))]
pub(crate) fn where_exe() -> PathBuf {
    system32_directory().join("where.exe")
}

#[cfg(any(windows, test))]
pub(crate) fn notepad_exe() -> PathBuf {
    system32_directory().join("notepad.exe")
}

#[cfg(any(windows, test))]
pub(crate) fn powershell_exe() -> PathBuf {
    system32_directory()
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
}

#[cfg(any(windows, test))]
pub(crate) fn explorer_exe() -> PathBuf {
    windows_directory().join("explorer.exe")
}

/// Drop `where.exe` results that come from the directory it searched first.
///
/// `where.exe` always searches its current directory before `PATH`, so a
/// result found there is whatever happened to be lying in that directory, not
/// an installed program. Only absolute paths outside `searched_first` remain.
#[cfg(any(windows, test))]
pub(crate) fn installed_where_results(
    results: impl IntoIterator<Item = PathBuf>,
    searched_first: &Path,
) -> Vec<PathBuf> {
    let same_directory = |path: &Path| {
        path.parent().is_some_and(|parent| {
            parent
                .as_os_str()
                .to_string_lossy()
                .trim_end_matches('\\')
                .eq_ignore_ascii_case(
                    searched_first
                        .as_os_str()
                        .to_string_lossy()
                        .trim_end_matches('\\'),
                )
        })
    };
    results
        .into_iter()
        .filter(|path| path.is_absolute() && !same_directory(path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_tools_are_absolute_paths_to_the_copies_windows_ships() {
        for tool in [
            reg_exe(),
            where_exe(),
            notepad_exe(),
            powershell_exe(),
            explorer_exe(),
        ] {
            assert!(tool.is_absolute(), "{}", tool.display());
            assert!(
                tool.starts_with(windows_directory()),
                "{} is outside the Windows directory",
                tool.display()
            );
            #[cfg(windows)]
            assert!(tool.is_file(), "{} does not exist", tool.display());
        }
        assert!(system32_directory().starts_with(windows_directory()));
    }

    #[test]
    fn where_results_from_its_own_directory_are_dropped() {
        let searched = Path::new(r"C:\Users\someone\Downloads");
        let kept = installed_where_results(
            [
                PathBuf::from(r"C:\Users\someone\Downloads\claude.exe"),
                PathBuf::from(r"c:\users\SOMEONE\downloads\claude.exe"),
                PathBuf::from(r"claude.exe"),
                PathBuf::from(r"C:\Users\someone\AppData\Local\AnthropicClaude\claude.exe"),
                PathBuf::from(r"C:\Users\someone\Downloads\nested\claude.exe"),
            ],
            searched,
        );
        assert_eq!(
            kept,
            [
                PathBuf::from(r"C:\Users\someone\AppData\Local\AnthropicClaude\claude.exe"),
                PathBuf::from(r"C:\Users\someone\Downloads\nested\claude.exe"),
            ]
        );
    }
}
