//! The broker icon's Unlink on a remote AI app, in the process that runs the
//! relay. The icon names either a linked app (an opaque entry of
//! [`crate::remote_grants`]) or, when that list is unavailable, a live session;
//! this resolves the name against state held here and revokes that one OAuth
//! grant with the same call the Remote panel's Unlink makes
//! ([`crate::relay::revoke_connector_grant`]). Nothing published to disk is
//! trusted for the grant, and nothing is ever revoked by client id.
//!
//! The relay's revoke succeeds even for a grant that does not exist, so its
//! answer alone proves nothing. "Unlinked" is said only when the grant was in
//! the account's list read just before the revoke and is gone from the list
//! read just after it.
use qc_relay_client::ConnectorRevoke;
use std::future::Future;
use std::time::Duration;

/// The relay request has no timeout of its own; the icon's action must end.
/// With the two list reads (8 s each) the worst case is 36 s.
const REVOKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Reported when the relay did not answer in time. The request may have
/// reached it, so the outcome is unknown, never "nothing was unlinked".
pub(crate) const REVOKE_TIMED_OUT: &str =
    "QC Remote Cloud did not answer in time, so it is not known whether the app was unlinked. The list will refresh.";
/// A 404 from the revoke route: only a relay deployed before the current
/// one answers it, when it could not store the revoke.
pub(crate) const REVOKE_STORAGE_ERROR: &str =
    "QC Remote Cloud could not process the unlink (storage error): try again";
/// The relay accepted the revoke but still lists the grant.
pub(crate) const STILL_LISTED: &str =
    "QC Remote Cloud accepted the unlink but still lists the app; check the Remote panel";
/// The revoke was sent but the list could not be read to confirm it.
pub(crate) const UNCONFIRMED: &str =
    "The unlink was sent, but the linked apps could not be read to confirm it. The list will refresh.";

/// What a remote Unlink did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteMcpUnlink {
    /// Listed before the revoke and gone after it: that app must sign in again.
    Unlinked,
    /// The account's list, read just before, no longer had it; nothing sent.
    NotListed,
    /// The named session is no longer live here; nothing was sent.
    SessionEnded,
    /// The session is live but the relay did not name its grant (a relay
    /// deployed before grants were published). Nothing was revoked.
    GrantUnknown,
    /// The linked-app list was fetched under an earlier pairing; nothing sent.
    StaleList,
}

/// The rule, with the lookup, the list read and the revoke injected.
///
/// `grant_of` answers from this process's own state: `None` for no such
/// session or entry, `Some(None)` for a session without a known grant.
/// `list` reads the account's grant ids now. Exactly one revoke is attempted,
/// for exactly that grant, with no retry and no other grant tried.
pub(crate) async fn unlink_verified<G, L, LF, R, RF>(
    id: &str,
    grant_of: G,
    mut list: L,
    revoke: R,
    timeout: Duration,
) -> Result<RemoteMcpUnlink, String>
where
    G: FnOnce(&str) -> Option<Option<String>>,
    L: FnMut() -> LF,
    LF: Future<Output = Result<Vec<String>, String>>,
    R: FnOnce(String) -> RF,
    RF: Future<Output = Result<ConnectorRevoke, String>>,
{
    let grant = match grant_of(id) {
        None => return Ok(RemoteMcpUnlink::SessionEnded),
        Some(None) => return Ok(RemoteMcpUnlink::GrantUnknown),
        Some(Some(grant)) => grant,
    };
    let before = list().await.map_err(|error| {
        format!("Could not read the linked apps before unlinking ({error}); nothing was unlinked")
    })?;
    if !before.contains(&grant) {
        return Ok(RemoteMcpUnlink::NotListed);
    }
    match tokio::time::timeout(timeout, revoke(grant.clone())).await {
        Err(_) => {
            let _ = list().await;
            return Err(REVOKE_TIMED_OUT.into());
        }
        Ok(Err(error)) => return Err(error),
        Ok(Ok(ConnectorRevoke::NotFound)) => return Err(REVOKE_STORAGE_ERROR.into()),
        Ok(Ok(ConnectorRevoke::Revoked)) => {}
    }
    match list().await {
        Ok(after) if !after.contains(&grant) => Ok(RemoteMcpUnlink::Unlinked),
        Ok(_) => Err(STILL_LISTED.into()),
        Err(_) => Err(UNCONFIRMED.into()),
    }
}

async fn revoke(grant: String) -> Result<ConnectorRevoke, String> {
    crate::relay::revoke_connector_grant(&grant)
        .await
        .map(|(_, outcome)| outcome)
}

fn run<G>(id: &str, grant_of: G) -> Result<RemoteMcpUnlink, String>
where
    G: FnOnce(&str) -> Option<Option<String>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start the unlink request: {error}"))?;
    runtime.block_on(unlink_verified(
        id,
        grant_of,
        crate::remote_grants::fetch,
        revoke,
        REVOKE_TIMEOUT,
    ))
}

/// Blocking entry for the icon's action thread: a live session, resolved
/// from this process's relay presence.
pub(crate) fn unlink_remote_session(session_id: &str) -> Result<RemoteMcpUnlink, String> {
    run(session_id, crate::host_presence::live_remote_grant)
}

/// Blocking entry for the icon's action thread: a linked app, resolved from
/// this process's own linked-apps list. Works with no live session. A list
/// fetched under another pairing than the current one resolves nothing.
pub(crate) fn unlink_linked_app(entry_id: &str) -> Result<RemoteMcpUnlink, String> {
    let resolved = crate::remote_grants::grant_of_entry(entry_id);
    if let Some((_, pairing)) = &resolved {
        if !same_pairing(pairing, crate::remote_grants::current_identity().as_deref()) {
            crate::remote_grants::clear();
            return Ok(RemoteMcpUnlink::StaleList);
        }
    }
    run(entry_id, |_| resolved.map(|(grant, _)| Some(grant)))
}

fn same_pairing(listed: &str, current: Option<&str>) -> bool {
    current == Some(listed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Relay {
        /// The account's grants, as each list read sees them.
        listed: RefCell<Vec<String>>,
        /// Whether a revoke really removes the grant from the list.
        removes: bool,
        answer: Result<ConnectorRevoke, String>,
        revoked: RefCell<Vec<String>>,
        reads: RefCell<usize>,
        fail_read: Option<usize>,
    }

    impl Relay {
        fn new(listed: &[&str]) -> Self {
            Self {
                listed: RefCell::new(listed.iter().map(|grant| (*grant).to_owned()).collect()),
                removes: true,
                answer: Ok(ConnectorRevoke::Revoked),
                revoked: RefCell::new(Vec::new()),
                reads: RefCell::new(0),
                fail_read: None,
            }
        }

        fn run(&self, id: &str) -> Result<RemoteMcpUnlink, String> {
            self.run_with(id, REVOKE_TIMEOUT, false)
        }

        fn run_with(
            &self,
            id: &str,
            timeout: Duration,
            hang: bool,
        ) -> Result<RemoteMcpUnlink, String> {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(unlink_verified(
                    id,
                    live,
                    || {
                        let read = {
                            let mut reads = self.reads.borrow_mut();
                            *reads += 1;
                            *reads
                        };
                        let result = if self.fail_read == Some(read) {
                            Err("down".to_string())
                        } else {
                            Ok(self.listed.borrow().clone())
                        };
                        async move { result }
                    },
                    |grant| {
                        self.revoked.borrow_mut().push(grant.clone());
                        if self.removes && self.answer.is_ok() {
                            self.listed.borrow_mut().retain(|listed| *listed != grant);
                        }
                        let answer = self.answer.clone();
                        async move {
                            if hang {
                                std::future::pending::<()>().await;
                            }
                            answer
                        }
                    },
                    timeout,
                ))
        }
    }

    /// Two sessions of one app, each under its own grant.
    fn live(session: &str) -> Option<Option<String>> {
        match session {
            "relay-a" => Some(Some("GrantAAAA1".into())),
            "relay-b" => Some(Some("GrantBBBB2".into())),
            "relay-old" => Some(None),
            _ => None,
        }
    }

    #[test]
    fn unlinked_only_when_listed_before_and_gone_after() {
        let relay = Relay::new(&["GrantAAAA1", "GrantBBBB2"]);
        assert_eq!(relay.run("relay-b"), Ok(RemoteMcpUnlink::Unlinked));
        assert_eq!(*relay.revoked.borrow(), ["GrantBBBB2"]);
        assert_eq!(*relay.listed.borrow(), ["GrantAAAA1"]);
        assert_eq!(*relay.reads.borrow(), 2, "read before and after");
    }

    #[test]
    fn a_grant_missing_from_the_fresh_list_is_not_revoked() {
        // The relay would answer 200 for it anyway; nothing is sent.
        let relay = Relay::new(&["GrantAAAA1"]);
        assert_eq!(relay.run("relay-b"), Ok(RemoteMcpUnlink::NotListed));
        assert!(relay.revoked.borrow().is_empty());
    }

    #[test]
    fn a_grant_still_listed_after_an_accepted_revoke_is_not_called_unlinked() {
        let relay = Relay {
            removes: false,
            ..Relay::new(&["GrantAAAA1"])
        };
        assert_eq!(relay.run("relay-a"), Err(STILL_LISTED.into()));
    }

    #[test]
    fn an_old_relays_404_is_a_storage_error_to_retry() {
        let relay = Relay {
            answer: Ok(ConnectorRevoke::NotFound),
            ..Relay::new(&["GrantAAAA1"])
        };
        assert_eq!(relay.run("relay-a"), Err(REVOKE_STORAGE_ERROR.into()));
        assert!(REVOKE_STORAGE_ERROR.contains("storage error"));
        assert!(REVOKE_STORAGE_ERROR.contains("try again"));
    }

    #[test]
    fn a_failure_is_reported_once_and_no_other_grant_is_tried() {
        let relay = Relay {
            answer: Err("QC Remote Cloud could not unlink that connector".into()),
            ..Relay::new(&["GrantAAAA1", "GrantBBBB2"])
        };
        assert_eq!(
            relay.run("relay-a"),
            Err("QC Remote Cloud could not unlink that connector".into())
        );
        assert_eq!(*relay.revoked.borrow(), ["GrantAAAA1"]);
    }

    #[test]
    fn an_unreadable_list_after_the_revoke_says_so_and_before_it_sends_nothing() {
        let after = Relay {
            fail_read: Some(2),
            ..Relay::new(&["GrantAAAA1"])
        };
        assert_eq!(after.run("relay-a"), Err(UNCONFIRMED.into()));
        let before = Relay {
            fail_read: Some(1),
            ..Relay::new(&["GrantAAAA1"])
        };
        assert!(before
            .run("relay-a")
            .unwrap_err()
            .contains("nothing was unlinked"));
        assert!(before.revoked.borrow().is_empty());
    }

    #[test]
    fn a_timeout_says_the_outcome_is_unknown() {
        let relay = Relay::new(&["GrantAAAA1"]);
        let outcome = relay.run_with("relay-a", Duration::from_millis(10), true);
        assert_eq!(outcome, Err(REVOKE_TIMED_OUT.into()));
        assert!(!REVOKE_TIMED_OUT.contains("nothing was unlinked"));
        assert!(REVOKE_TIMED_OUT.contains("not known"));
        assert!(REVOKE_TIMED_OUT.contains("refresh"));
        assert_eq!(*relay.reads.borrow(), 2, "the list is read again");
    }

    #[test]
    fn no_grant_or_no_session_revokes_nothing() {
        for (session, expected) in [
            ("relay-old", RemoteMcpUnlink::GrantUnknown),
            ("relay-gone", RemoteMcpUnlink::SessionEnded),
        ] {
            let relay = Relay::new(&["GrantAAAA1"]);
            assert_eq!(relay.run(session), Ok(expected));
            assert!(relay.revoked.borrow().is_empty(), "{session}");
            assert_eq!(*relay.reads.borrow(), 0);
        }
    }

    #[test]
    fn a_list_from_another_pairing_is_stale() {
        assert!(same_pairing("e|d1", Some("e|d1")));
        assert!(!same_pairing("e|d1", Some("e|d2")));
        assert!(!same_pairing("e|d1", None), "unpaired since");
    }
}
