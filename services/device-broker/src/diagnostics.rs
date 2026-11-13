//! Diagnostics for runtime threads that must never wait on stderr.
//!
//! `eprintln!` is a blocking write that panics on failure. The broker's stderr
//! is whatever its parent handed it: a file or null for the app's own launches,
//! but a pipe for conformance runs, scripts and hosts. A parent that never
//! drains that pipe blocks the first writer once the buffer is full, and a
//! parent that closed its end makes `eprintln!` panic. On the device worker,
//! the accept loop or the orphan guard either outcome stops the USB session or
//! keeps the device after its owner is gone.
//!
//! [`diag!`] formats the line on the calling thread and hands it to one
//! process-lifetime thread through a bounded queue with `try_send`. A full
//! queue drops the line and counts it; the writer thread alone ever touches
//! stderr, ignores write errors, and reports the count when it can write again.
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread;

/// Lines that may wait for the writer thread before further ones are dropped.
const QUEUE_LINES: usize = 256;

/// A bounded line queue in front of one writer thread.
pub(crate) struct Logger {
    sender: mpsc::SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl Logger {
    /// Start the writer thread over `sink`. `None` when no thread could start.
    pub(crate) fn with_sink(sink: Box<dyn Write + Send>) -> Option<Self> {
        let (sender, lines) = mpsc::sync_channel::<String>(QUEUE_LINES);
        let dropped = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&dropped);
        thread::Builder::new()
            .name("qc-broker-diagnostics".into())
            .spawn(move || {
                let mut sink = sink;
                while let Ok(line) = lines.recv() {
                    let missed = counted.swap(0, Ordering::Relaxed);
                    if missed != 0 {
                        let _ = writeln!(sink, "({missed} diagnostic line(s) were dropped)");
                    }
                    let _ = writeln!(sink, "{line}");
                }
            })
            .ok()?;
        Some(Self { sender, dropped })
    }

    /// Queue one line. Never blocks and never panics; `false` when the line
    /// was dropped because the queue is full or the writer is gone.
    pub(crate) fn log(&self, line: String) -> bool {
        match self.sender.try_send(line) {
            Ok(()) => true,
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }
}

fn global() -> Option<&'static Logger> {
    static LOGGER: OnceLock<Option<Logger>> = OnceLock::new();
    LOGGER
        .get_or_init(|| Logger::with_sink(Box::new(StderrSink)))
        .as_ref()
}

/// Stderr without `eprintln!`'s panic: a failed write is an `Err`.
struct StderrSink;

impl Write for StderrSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::stderr().lock().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        io::stderr().lock().flush()
    }
}

#[doc(hidden)]
pub(crate) fn emit(arguments: std::fmt::Arguments<'_>) {
    if let Some(logger) = global() {
        logger.log(arguments.to_string());
    }
}

/// `eprintln!` for threads that must not block or panic on stderr.
macro_rules! diag {
    ($($argument:tt)*) => {
        $crate::diagnostics::emit(format_args!($($argument)*))
    };
}
pub(crate) use diag;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// A sink that never returns, as a pipe nobody drains.
    struct StuckSink(Arc<(Mutex<bool>, Condvar)>);

    impl Write for StuckSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let (released, wake) = &*self.0;
            let mut released = released.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_stuck_stderr_never_blocks_the_caller() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let logger = Logger::with_sink(Box::new(StuckSink(Arc::clone(&gate)))).unwrap();
        let started = Instant::now();
        let accepted = (0..1000)
            .filter(|index| logger.log(format!("line {index}")))
            .count();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "diagnostics waited on the sink: {:?}",
            started.elapsed()
        );
        // The writer holds one line and the queue holds the rest of its bound.
        assert!(accepted <= QUEUE_LINES + 1, "{accepted}");
        // (The drop count is reported and reset by the writer, so it is not asserted.)
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    /// A sink whose every write fails, as a closed pipe: no panic, no stop.
    struct ClosedSink(mpsc::Sender<()>);

    impl Write for ClosedSink {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            let _ = self.0.send(());
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_stderr_is_ignored_and_the_writer_keeps_running() {
        let (attempts, seen) = mpsc::channel();
        let logger = Logger::with_sink(Box::new(ClosedSink(attempts))).unwrap();
        for index in 0..3 {
            assert!(logger.log(format!("line {index}")));
            seen.recv_timeout(Duration::from_secs(5))
                .expect("the writer thread survived the failed write");
        }
    }
}
