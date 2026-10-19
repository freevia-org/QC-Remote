//! One process-generation verdict shared by native startup and lease admission.
//! Status/polling uses `native`; only an explicit recovery creates an admission.
use std::path::Path;

/// Preparation consumes the original budget. No election or start callback is
/// admitted once that absolute deadline has elapsed.
pub fn after_preparation<T>(
    deadline: std::time::Instant,
    now: std::time::Instant,
    admit: impl FnOnce(std::time::Duration) -> T,
) -> Result<T, &'static str> {
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Err(
            "Broker recovery budget expired during owner verification; no new election was started",
        );
    }
    Ok(admit(remaining))
}

/// Launchability check only, NEVER owner identity evidence. These entrypoints
/// implement the exact early helper mode. Unknown/renamed hosts do not spawn.
pub fn current_helper() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let path = std::env::current_exe().ok()?;
        if matches!(
            path.file_stem()?.to_str()?,
            "qc-remote"
                | "qc-device-broker"
                | "qc-device-broker-x86_64-pc-windows-msvc"
                | "qc-device-broker-x86_64-pc-windows-gnu"
        ) {
            return Some(path);
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assessment {
    Same,
    GoneOrDifferent,
    Unknown,
}

pub fn from_facts(alive: bool, recorded: Option<u64>, current: Option<u64>) -> Assessment {
    if !alive {
        return Assessment::GoneOrDifferent;
    }
    match (recorded, current) {
        (Some(recorded), Some(current)) if recorded == current => Assessment::Same,
        (Some(_), Some(_)) => Assessment::GoneOrDifferent,
        _ => Assessment::Unknown,
    }
}

pub fn native(pid: u32, recorded: Option<u64>) -> Assessment {
    if pid == 0 {
        return Assessment::Unknown;
    }
    if !crate::process::os_process_is_alive(pid) {
        return Assessment::GoneOrDifferent;
    }
    from_facts(true, recorded, crate::process::os_process_created_at(pid))
}

/// At most one independent query, for exactly one observed generation. The
/// successful Different proof can survive a later denied native read; Same and
/// Unknown are never promoted. Callers re-read their document under ownership.
pub struct RecoveryAdmission {
    observed: Option<(u32, Option<u64>, Assessment)>,
}

impl RecoveryAdmission {
    pub fn none() -> Self {
        Self { observed: None }
    }

    pub fn prepare(pid: u32, recorded: Option<u64>, helper: Option<&Path>) -> Self {
        Self::prepare_with(pid, recorded, native(pid, recorded), || {
            #[cfg(windows)]
            {
                helper.map_or(Assessment::Unknown, |helper| {
                    match qc_windows_process::creation_probe::assess_at_recovery_admission(
                        helper, pid, recorded,
                    ) {
                        qc_windows_process::creation_probe::Assessment::Same => Assessment::Same,
                        qc_windows_process::creation_probe::Assessment::GoneOrDifferent => {
                            Assessment::GoneOrDifferent
                        }
                        qc_windows_process::creation_probe::Assessment::Unknown => {
                            Assessment::Unknown
                        }
                    }
                })
            }
            #[cfg(not(windows))]
            {
                let _ = helper;
                Assessment::Unknown
            }
        })
    }

    /// The lookup is one synchronous independent observation, not a background
    /// retry. Injectable facts let callers verify policy without OS side effects.
    pub fn prepare_with(
        pid: u32,
        recorded: Option<u64>,
        native: Assessment,
        independent: impl FnOnce() -> Assessment,
    ) -> Self {
        let verdict = if pid != 0 && recorded.is_some() && native == Assessment::Unknown {
            independent()
        } else {
            native
        };
        Self::observed(pid, recorded, verdict)
    }

    fn observed(pid: u32, recorded: Option<u64>, verdict: Assessment) -> Self {
        Self {
            observed: Some((pid, recorded, verdict)),
        }
    }

    pub fn assess(&self, pid: u32, recorded: Option<u64>) -> Assessment {
        self.revalidate(pid, recorded, native(pid, recorded))
    }

    pub fn revalidate(&self, pid: u32, recorded: Option<u64>, current: Assessment) -> Assessment {
        match (current, self.observed) {
            // Contradictory evidence must not authorize retirement.
            (Assessment::Same, Some((old_pid, old_time, Assessment::GoneOrDifferent)))
                if (pid, recorded) == (old_pid, old_time) =>
            {
                Assessment::Unknown
            }
            (Assessment::Unknown, Some((old_pid, old_time, Assessment::GoneOrDifferent)))
                if (pid, recorded) == (old_pid, old_time) =>
            {
                Assessment::GoneOrDifferent
            }
            _ => current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_recovery_preparation_uses_original_deadline_without_late_effects() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let deadline = start + Duration::from_secs(2);
        for now in [deadline, deadline + Duration::from_secs(1)] {
            assert!(
                after_preparation(deadline, now, |_| panic!("expired admission ran effects"))
                    .is_err()
            );
        }
        assert_eq!(
            after_preparation(deadline, start + Duration::from_millis(750), |remaining| {
                remaining
            })
            .unwrap(),
            Duration::from_millis(1250)
        );
    }
    #[test]
    fn native_identity_unknown_is_not_dead() {
        assert_eq!(from_facts(true, Some(7), None), Assessment::Unknown);
        assert_eq!(from_facts(true, None, Some(8)), Assessment::Unknown);
        assert_eq!(from_facts(false, None, None), Assessment::GoneOrDifferent);
        assert_eq!(from_facts(true, Some(7), Some(7)), Assessment::Same);
        assert_eq!(
            from_facts(true, Some(7), Some(8)),
            Assessment::GoneOrDifferent
        );
    }
    #[test]
    fn independent_proof_is_scoped_to_exact_observed_generation() {
        let proof = RecoveryAdmission::observed(7, Some(10), Assessment::GoneOrDifferent);
        assert_eq!(
            proof.revalidate(7, Some(10), Assessment::Unknown),
            Assessment::GoneOrDifferent
        );
        assert_eq!(
            proof.revalidate(7, Some(11), Assessment::Unknown),
            Assessment::Unknown
        );
        assert_eq!(
            proof.revalidate(8, Some(10), Assessment::Unknown),
            Assessment::Unknown
        );
        assert_eq!(
            proof.revalidate(7, Some(10), Assessment::Same),
            Assessment::Unknown
        );
    }
    #[test]
    fn matching_or_failed_query_never_promotes_unknown() {
        for evidence in [Assessment::Same, Assessment::Unknown] {
            let proof = RecoveryAdmission::observed(7, Some(10), evidence);
            assert_eq!(
                proof.revalidate(7, Some(10), Assessment::Unknown),
                Assessment::Unknown
            );
        }
    }
    #[test]
    fn independent_lookup_runs_once_not_during_revalidation() {
        let count = std::cell::Cell::new(0);
        let proof = RecoveryAdmission::prepare_with(42, Some(7), Assessment::Unknown, || {
            count.set(count.get() + 1);
            Assessment::GoneOrDifferent
        });
        for _ in 0..32 {
            assert_eq!(
                proof.revalidate(42, Some(7), Assessment::Unknown),
                Assessment::GoneOrDifferent
            );
        }
        assert_eq!(count.get(), 1);
    }
}
