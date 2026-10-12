//! Linux process signals enter the shared worker/descriptor shutdown path.
//! signal-hook does only async-signal-safe notification in the signal handler;
//! broker work happens on an owned thread, never in that handler.
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::{Handle, Signals};
use std::thread::{self, JoinHandle};

pub(crate) struct Pending(Signals);

impl Pending {
    pub(crate) fn new() -> Result<Self, String> {
        Signals::new([SIGINT, SIGTERM])
            .map(Self)
            .map_err(|error| format!("Could not register broker shutdown signals: {error}"))
    }

    pub(crate) fn watch(self, shutdown: impl FnOnce() + Send + 'static) -> Result<Watch, String> {
        let mut signals = self.0;
        let handle = signals.handle();
        let thread = thread::Builder::new()
            .name("qc-broker-signals".into())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    shutdown();
                }
            })
            .map_err(|error| format!("Could not start broker shutdown watcher: {error}"))?;
        Ok(Watch {
            handle,
            thread: Some(thread),
        })
    }
}

pub(crate) struct Watch {
    handle: Handle,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn failed_or_returning_serve_closes_and_joins_its_signal_watcher() {
        let called = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&called);
        let watch = Pending::new()
            .unwrap()
            .watch(move || {
                observed.store(true, Ordering::SeqCst);
            })
            .unwrap();
        let finished = watch.thread.as_ref().unwrap().thread().id();
        assert_ne!(finished, thread::current().id());
        drop(watch); // Closes the blocking iterator and joins without a signal.
        assert!(!called.load(Ordering::SeqCst));
    }
}
