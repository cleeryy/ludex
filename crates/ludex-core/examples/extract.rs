//! CLI used by `scripts/bench-extract.sh` (#232).
//!
//! ```text
//! cargo run -q -p ludex-core --example extract -- <archive> <dest> [options]
//!
//!   --workers N        ZIP workers (0 = auto, capped at MAX_ZIP_WORKERS)
//!   --cancel-after S   raise the cancel flag after S seconds
//!   --progress         print one JSON line per progress callback
//! ```
//!
//! Prints a single JSON object on the last line (`event: "done"` or
//! `event: "error"`) so the benchmark can parse the run without scraping
//! human-readable output.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ludex_core::archive::{extract_archive, CancelCheck, ExtractOptions, ExtractProgress};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut archive: Option<PathBuf> = None;
    let mut dest: Option<PathBuf> = None;
    let mut workers = 0_usize;
    let mut cancel_after: Option<f64> = None;
    let mut verbose = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workers" => match args.next().and_then(|value| value.parse().ok()) {
                Some(value) => workers = value,
                None => return usage("--workers needs a number"),
            },
            "--cancel-after" => match args.next().and_then(|value| value.parse().ok()) {
                Some(value) => cancel_after = Some(value),
                None => return usage("--cancel-after needs seconds"),
            },
            "--progress" => verbose = true,
            "--help" | "-h" => return usage(""),
            other if other.starts_with("--") => return usage("unknown option"),
            other => {
                if archive.is_none() {
                    archive = Some(PathBuf::from(other));
                } else if dest.is_none() {
                    dest = Some(PathBuf::from(other));
                } else {
                    return usage("too many arguments");
                }
            }
        }
    }

    let (Some(archive), Some(dest)) = (archive, dest) else {
        return usage("archive and destination are required");
    };

    let flag = Arc::new(AtomicBool::new(false));
    let mut options = ExtractOptions {
        zip_workers: workers,
        ..ExtractOptions::default()
    };
    if let Some(seconds) = cancel_after {
        options.cancel = Some(CancelCheck::from_flag(Arc::clone(&flag)));
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs_f64(seconds));
            flag.store(true, Ordering::Relaxed);
        });
    }

    let started = Instant::now();
    let mut last = ExtractProgress::default();
    let result = extract_archive(&archive, &dest, &options, &mut |progress| {
        if verbose {
            println!(
                "{{\"event\":\"progress\",\"entriesDone\":{},\"entriesTotal\":{},\"bytesDone\":{},\"bytesTotal\":{},\"bytesPerSecond\":{},\"etaSeconds\":{}}}",
                progress.entries_done,
                progress.entries_total,
                progress.bytes_done,
                progress.bytes_total,
                progress.bytes_per_second,
                progress
                    .eta_seconds
                    .map(|seconds| seconds.to_string())
                    .unwrap_or_else(|| "null".to_string()),
            );
        }
        last = progress;
    });
    let seconds = started.elapsed().as_secs_f64();

    match result {
        Ok(report) => {
            println!(
                "{{\"event\":\"done\",\"files\":{},\"bytes\":{},\"seconds\":{seconds:.3},\"bytesPerSecond\":{},\"workers\":{workers}}}",
                report.files,
                report.bytes,
                (report.bytes as f64 / seconds.max(0.001)) as u64,
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            let cancelled = matches!(error, ludex_core::archive::ArchiveError::Cancelled);
            println!(
                "{{\"event\":\"error\",\"cancelled\":{cancelled},\"seconds\":{seconds:.3},\"bytesDone\":{},\"error\":{}}}",
                last.bytes_done,
                serde_json::Value::String(error.to_string()),
            );
            if cancelled {
                ExitCode::from(3)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn usage(message: &str) -> ExitCode {
    if !message.is_empty() {
        eprintln!("error: {message}");
    }
    eprintln!("usage: extract <archive> <dest> [--workers N] [--cancel-after SECS] [--progress]");
    ExitCode::from(2)
}
