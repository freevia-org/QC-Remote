//! Error-aware loading for the access policy shared by GUI and background host.
//!
//! A missing preference is distinct from a credential-store error or invalid
//! value so migration callers can apply a conservative policy.

use qc_relay_client::AccessMode;

/// Which persisted keyring entry is being read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Store {
    Current,
    Legacy,
}

/// Reads the current preference first, consulting the legacy key only when
/// the current key is genuinely absent. `Err` means the store could not answer
/// (including a cancelled unlock prompt); it never means `NoEntry`.
pub(crate) fn read_with(
    mut read: impl FnMut(Store) -> Result<Option<String>, String>,
) -> Result<Option<AccessMode>, String> {
    match read(Store::Current)? {
        Some(value) => AccessMode::parse(&value).map(Some),
        None => match read(Store::Legacy)? {
            Some(value) => AccessMode::parse(&value).map(Some),
            None => Ok(None),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_pair(
        current: Result<Option<&'static str>, &'static str>,
        legacy: Result<Option<&'static str>, &'static str>,
    ) -> impl FnMut(Store) -> Result<Option<String>, String> {
        move |store| {
            let value = match store {
                Store::Current => current,
                Store::Legacy => legacy,
            };
            value
                .map(|value| value.map(str::to_owned))
                .map_err(str::to_owned)
        }
    }

    #[test]
    fn loads_a_stored_restriction_and_uses_legacy_only_when_current_is_missing() {
        assert_eq!(
            read_with(read_pair(Ok(Some("read-only")), Ok(Some("full")))),
            Ok(Some(AccessMode::ReadOnly))
        );
        assert_eq!(
            read_with(read_pair(Ok(None), Ok(Some("modify")))),
            Ok(Some(AccessMode::Modify))
        );
    }

    #[test]
    fn a_current_store_error_never_falls_through_to_legacy_full() {
        let mut calls = Vec::new();
        let result = read_with(|store| {
            calls.push(store);
            match store {
                Store::Current => Err("unlock cancelled".into()),
                Store::Legacy => Ok(Some("full".into())),
            }
        });
        assert!(result.is_err());
        assert_eq!(calls, [Store::Current]);
    }

    #[test]
    fn invalid_current_or_legacy_values_are_errors_not_full_defaults() {
        assert!(read_with(read_pair(Ok(Some("invalid")), Ok(Some("full")))).is_err());
    }

    #[test]
    fn both_missing_remains_distinct_for_migration_policy() {
        let mut calls = Vec::new();
        let result = read_with(|store| {
            calls.push(store);
            Ok(None)
        });
        assert_eq!(result, Ok(None));
        assert_eq!(calls, [Store::Current, Store::Legacy]);
    }
}
