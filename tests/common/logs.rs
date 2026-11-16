//! Captures the app's log output in-process, as the binary would print it.
//!
//! One global subscriber serves the whole test binary, because tracing caches
//! per call site whether anyone listens, which thread-scoped subscribers
//! confuse when tests run in parallel. It writes each line to the capture of
//! the thread that logged it. With `#[tokio::test]`'s single-threaded
//! runtime, the app's tasks run on the test's thread.

use std::cell::RefCell;
use std::io::Write;
use std::sync::{Arc, Mutex, Once};

thread_local! {
    static CAPTURE: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
}

/// Log lines written on this thread while the capture is alive.
pub struct Logs {
    buffer: Arc<Mutex<Vec<u8>>>,
}

pub fn capture() -> Logs {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(|| Writer)
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("no other subscriber");
    });
    let buffer = Arc::new(Mutex::new(Vec::new()));
    CAPTURE.set(Some(buffer.clone()));
    Logs { buffer }
}

impl Drop for Logs {
    fn drop(&mut self) {
        CAPTURE.set(None);
    }
}

impl Logs {
    pub fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.buffer.lock().unwrap())
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The lines containing `needle`.
    pub fn matching(&self, needle: &str) -> Vec<String> {
        self.lines()
            .into_iter()
            .filter(|line| line.contains(needle))
            .collect()
    }
}

/// Writes to the current thread's capture, if any, and drops the line
/// otherwise.
struct Writer;

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        CAPTURE.with_borrow(|capture| {
            if let Some(buffer) = capture {
                buffer.lock().unwrap().extend_from_slice(buf);
            }
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
