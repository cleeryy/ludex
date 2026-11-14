//! Tracing output for the desktop app (#228).
//!
//! A release Windows build is `windows_subsystem = "windows"`: stderr goes
//! nowhere, so a GILRS/overlay/window failure was invisible in the field. Every
//! line is still written to stderr for a console run, and mirrored to
//! `{appDataDir}/logs/ludex.log` once the app data folder is known.
//!
//! The file handle is installed from `setup`, after Tauri resolves the app data
//! folder; lines emitted before that (plugin startup) stay on stderr only.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Appended log file, opened once. `None` until `install` runs.
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Relative to the app data folder.
pub const LOG_SUBDIR: &str = "logs";
pub const LOG_NAME: &str = "ludex.log";

/// Create the log folder, open (append) the file and enable the mirror.
///
/// Never fatal: a read-only or missing app data folder keeps stderr logging.
pub fn install(app_data: &Path) -> Option<PathBuf> {
    let dir = app_data.join(LOG_SUBDIR);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "ludex: could not create the log folder {}: {error}",
            dir.display()
        );
        return None;
    }
    let path = dir.join(LOG_NAME);
    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "ludex: could not open the log file {}: {error}",
                path.display()
            );
            return None;
        }
    };
    match LOG_FILE.lock() {
        Ok(mut slot) => {
            *slot = Some(file);
            Some(path)
        }
        Err(error) => {
            eprintln!("ludex: the log file mutex is poisoned: {error}");
            None
        }
    }
}

/// `MakeWriter` used by the tracing subscriber: stderr plus the log file.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriter {
    type Writer = LogSink;

    fn make_writer(&'a self) -> Self::Writer {
        LogSink
    }
}

/// One write target: mirrors every buffer to stderr and the open file.
#[derive(Debug)]
pub struct LogSink;

impl Write for LogSink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        // stderr is the console/debugger stream; the file is the field evidence.
        let _ = io::stderr().write_all(buffer);
        if let Ok(mut slot) = LOG_FILE.lock() {
            if let Some(file) = slot.as_mut() {
                let _ = file.write_all(buffer);
            }
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Ok(mut slot) = LOG_FILE.lock() {
            if let Some(file) = slot.as_mut() {
                let _ = file.flush();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installing_creates_the_log_folder_and_accepts_writes() {
        let dir = std::env::temp_dir().join(format!(
            "ludex-logging-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        let path = install(&dir).expect("the log file must open in a writable temp folder");
        assert!(path.ends_with("logs/ludex.log") || path.ends_with("logs\\ludex.log"));

        let mut sink = LogSink;
        sink.write_all(b"overlay listener started\n").unwrap();
        sink.flush().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("overlay listener started"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
