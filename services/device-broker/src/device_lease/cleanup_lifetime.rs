#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
#[cfg(test)]
use std::sync::{Mutex, Weak};

pub(super) const CLEANUP_LIFETIME_LIMIT: usize = 64;

/// Admission includes active lease generations and deferred cleanup through
/// exact cleanup acknowledgement. It is a software process limit, not a
/// firmware/device limit or a promise to cancel started OS filesystem calls.
pub(super) struct CleanupLimiter {
    limit: usize,
    count: AtomicUsize,
}
pub(super) struct CleanupPermit {
    limiter: Arc<CleanupLimiter>,
    paths: Vec<PathBuf>,
}
#[cfg(test)]
fn activity() -> &'static Mutex<Vec<Weak<CleanupPermit>>> {
    static ACTIVITY: OnceLock<Mutex<Vec<Weak<CleanupPermit>>>> = OnceLock::new();
    ACTIVITY.get_or_init(|| Mutex::new(Vec::new()))
}
impl CleanupPermit {
    pub(super) fn covers_paths(&self, paths: &[PathBuf]) -> bool {
        paths.iter().all(|path| self.paths.contains(path))
    }
}
impl Drop for CleanupPermit {
    fn drop(&mut self) {
        self.limiter.count.fetch_sub(1, Ordering::AcqRel);
    }
}
impl CleanupLimiter {
    pub(super) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            count: AtomicUsize::new(0),
        })
    }
    pub(super) fn production() -> Arc<Self> {
        #[cfg(not(test))]
        {
            static LIMITER: OnceLock<Arc<CleanupLimiter>> = OnceLock::new();
            Arc::clone(LIMITER.get_or_init(|| Self::new(CLEANUP_LIFETIME_LIMIT)))
        }
        #[cfg(test)]
        {
            // Test cohorts use the production implementation without parallel
            // test contention. Explicit injection can choose a smaller limit.
            thread_local! {
                static LIMITER: Arc<CleanupLimiter> = CleanupLimiter::new(CLEANUP_LIFETIME_LIMIT);
            }
            LIMITER.with(Arc::clone)
        }
    }
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    pub(super) fn reserve(self: &Arc<Self>, paths: Vec<PathBuf>) -> Result<Arc<CleanupPermit>, ()> {
        self.count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < self.limit).then_some(count + 1)
            })
            .map_err(|_| ())?;
        let permit = Arc::new(CleanupPermit {
            limiter: Arc::clone(self),
            paths,
        });
        // Register before any caller can touch lease paths. Weak observations
        // retain neither the permit nor its limiter and are pruned on use.
        #[cfg(test)]
        {
            let mut observed = activity().lock().unwrap_or_else(|p| p.into_inner());
            observed.retain(|entry| entry.strong_count() != 0);
            observed.push(Arc::downgrade(&permit));
        }
        Ok(permit)
    }
}
#[cfg(test)]
fn path_under(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
        let root = root.to_string_lossy().replace('/', "\\").to_lowercase();
        let root = root.trim_end_matches('\\');
        path == root || path.starts_with(&format!("{root}\\"))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}
#[cfg(test)]
pub(super) fn root_has_activity(root: &Path) -> bool {
    let mut observed = activity().lock().unwrap_or_else(|p| p.into_inner());
    observed.retain(|entry| entry.strong_count() != 0);
    observed
        .iter()
        .filter_map(Weak::upgrade)
        .any(|permit| permit.paths.iter().any(|path| path_under(path, root)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_lifetime_capacity_tracks_active_queued_and_inflight_clones_until_ack() {
        let limiter = CleanupLimiter::new(1);
        let root = std::env::temp_dir().join(format!("qc-cleanup-cap-{}", uuid::Uuid::new_v4()));
        let permit = limiter
            .reserve(vec![root.join("device-owner.json")])
            .unwrap();
        let native = Arc::clone(&permit);
        drop(permit);
        assert!(root_has_activity(&root));
        assert!(limiter.reserve(vec![root.join("other.json")]).is_err());
        // The same exact capability moves to queued then in-flight ownership.
        let mut queued = Some(native);
        let inflight = queued.take().unwrap();
        assert!(root_has_activity(&root));
        assert!(limiter.reserve(vec![root.join("other.json")]).is_err());
        drop(inflight);
        assert!(!root_has_activity(&root));
        assert_eq!(limiter.count.load(Ordering::Acquire), 0);
        let next = limiter.reserve(vec![root.join("other.json")]).unwrap();
        drop(next);
    }
}
