//! Logging to a file in the data directory, with secrets scrubbed on the way out.
//!
//! The code never logs secrets, SQL text, prompts or result data in the first place. The
//! `RedactingWriter` is the safety net for error messages from drivers and servers that might
//! echo a password or API key.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use application::redact::redact;

/// Rotate the log at startup once it grows beyond this.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Applies `redact` to every line before it reaches the inner writer.
pub struct RedactingWriter<W: Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }

    fn emit(&mut self, bytes: &[u8]) -> io::Result<()> {
        let text = String::from_utf8_lossy(bytes);
        self.inner.write_all(redact(&text).as_bytes())
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            self.emit(&line)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        // A final line without a newline must still be scrubbed, not dropped or leaked raw.
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            let _ = self.emit(&rest);
        }
        let _ = self.inner.flush();
    }
}

#[derive(Clone)]
struct SharedFile(Arc<Mutex<File>>);

impl Write for SharedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .flush()
    }
}

/// Starts logging to `<dir>/fathom.log` (level from `FATHOM_LOG`, default `info`). Failure to
/// log is never fatal: the app runs without a log instead.
pub fn init(dir: &Path) {
    let path = dir.join("fathom.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("fathom.log.1"));
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        eprintln!(
            "could not open the log file {}; continuing without a log",
            path.display()
        );
        return;
    };
    let shared = SharedFile(Arc::new(Mutex::new(file)));
    let filter = tracing_subscriber::EnvFilter::try_from_env("FATHOM_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || RedactingWriter::new(shared.clone()))
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Buffer {
        fn text(&self) -> String {
            String::from_utf8(
                self.0
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone(),
            )
            .unwrap_or_default()
        }
    }

    #[test]
    fn secrets_are_scrubbed_even_when_a_line_arrives_in_pieces() {
        let out = Buffer::default();
        {
            let mut w = RedactingWriter::new(out.clone());
            w.write_all(b"connect failed: pass").expect("write");
            w.write_all(b"word=hunter2 host=db\nnext line sk-abcdef1234567890\n")
                .expect("write");
        }
        let text = out.text();
        assert!(
            !text.contains("hunter2") && !text.contains("sk-abcdef"),
            "{text}"
        );
        assert!(
            text.contains("password=[redacted] host=db") && text.contains("next line [redacted]"),
            "{text}"
        );
    }

    #[test]
    fn a_final_line_without_a_newline_is_scrubbed_on_drop() {
        let out = Buffer::default();
        {
            let mut w = RedactingWriter::new(out.clone());
            w.write_all(b"token=abc123xyz").expect("write");
        }
        assert_eq!(out.text(), "token=[redacted]");
    }

    #[test]
    fn ordinary_lines_pass_through_unchanged() {
        let out = Buffer::default();
        {
            let mut w = RedactingWriter::new(out.clone());
            w.write_all(b"INFO query finished rows=3\n").expect("write");
        }
        assert_eq!(out.text(), "INFO query finished rows=3\n");
    }

    #[test]
    fn a_real_subscriber_writing_through_it_never_emits_a_secret() {
        let out = Buffer::default();
        let writer = out.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || RedactingWriter::new(writer.clone()))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                error = "server said: password=hunter2 for postgres://bob:s3cret@db/app",
                "connect failed"
            );
            tracing::info!(key = "Bearer abcdefgh12345678", "request");
        });
        let text = out.text();
        for secret in ["hunter2", "s3cret", "abcdefgh12345678"] {
            assert!(!text.contains(secret), "leaked `{secret}`: {text}");
        }
        assert!(
            text.contains("connect failed") && text.contains("[redacted]"),
            "{text}"
        );
    }

    #[test]
    fn init_writes_a_log_file_and_rotates_a_big_one() {
        let dir = std::env::temp_dir().join(format!("fathom-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("fathom.log"),
            vec![b'x'; (MAX_LOG_BYTES + 1) as usize],
        )
        .expect("big log");
        init(&dir);
        assert!(
            dir.join("fathom.log.1").exists(),
            "the oversized log was rotated"
        );
        assert!(dir.join("fathom.log").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
