//! `ludex-backup`: backup, restore and media migration CLI (#49).
//!
//! ```text
//! ludex-backup backup   [--data-dir DIR] [--media-dir DIR] [--saves-dir DIR]
//!                       [--out FILE|DIR] [--keep N] [--s3] [--json]
//! ludex-backup restore  --archive FILE [--data-dir DIR] [--media-dir DIR]
//!                       [--saves-dir DIR] [--force] [--dry-run] [--json]
//! ludex-backup migrate-media [--from DIR] [--delete-source] [--json]
//! ```
//!
//! Defaults come from `LUDEX_DATA_DIR`, `LUDEX_MEDIA_DIR`, `LUDEX_BACKUP_OUT`
//! and `LUDEX_BACKUP_KEEP`. `--s3` uploads the archive with the backend
//! configured through `LUDEX_STORAGE_BACKEND=s3` and `LUDEX_S3_*`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use ludex_server::backup::{
    create_backup, migrate_media, restore_backup, BackupRequest, RestoreRequest,
};
use ludex_server::storage::{StorageConfig, StorageKind};
use serde_json::json;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ludex-backup: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let Some(command) = arguments.next() else {
        print_usage();
        anyhow::bail!("missing command");
    };
    let flags = Flags::parse(arguments)?;

    match command.as_str() {
        "backup" => run_backup(flags),
        "restore" => run_restore(flags),
        "migrate-media" | "migrate_media" => run_migrate(flags),
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        other => {
            print_usage();
            anyhow::bail!("unknown command '{other}'")
        }
    }
}

fn run_backup(flags: Flags) -> Result<()> {
    flags.reject_unknown(
        &["data-dir", "media-dir", "saves-dir", "out", "keep"],
        &["json", "s3"],
    )?;
    let data_dir = flags.path_or_env("data-dir", "LUDEX_DATA_DIR", "./data")?;
    let media_dir = flags.path_or_env("media-dir", "LUDEX_MEDIA_DIR", "./media")?;
    let saves_dir = flags.optional_path("saves-dir");
    let out = flags.path_or_env("out", "LUDEX_BACKUP_OUT", "./backups")?;
    let keep = match flags.value("keep") {
        Some(value) => Some(
            value
                .parse::<usize>()
                .context("--keep must be an integer")?,
        ),
        None => flags
            .env_value("LUDEX_BACKUP_KEEP")
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("LUDEX_BACKUP_KEEP must be an integer")?,
    };

    let request = BackupRequest {
        data_dir,
        media_dir: Some(media_dir),
        saves_dir,
        out,
        keep,
    };
    let runtime = tokio::runtime::Runtime::new().context("build tokio runtime")?;
    let report = runtime.block_on(create_backup(&request))?;

    if flags.flag("s3") {
        let storage = s3_storage()?;
        let name = report
            .archive
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .context("archive has no file name")?;
        let key = format!("backups/{name}");
        let uploaded = runtime.block_on(storage.put_file(&key, &report.archive))?;
        if flags.flag("json") {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "archive": report.archive,
                    "files": report.files,
                    "bytes": report.bytes,
                    "components": report.components,
                    "pruned": report.pruned,
                    "uploaded": { "key": key, "bytes": uploaded },
                }))?
            );
        } else {
            println!("uploaded {key} ({uploaded} bytes)");
        }
    } else if flags.flag("json") {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "backup written to {} ({} files, {} bytes, components: {})",
            report.archive.display(),
            report.files,
            report.bytes,
            report.components.join(", ")
        );
    }
    if !report.pruned.is_empty() {
        for path in &report.pruned {
            println!("pruned {}", path.display());
        }
    }
    Ok(())
}

fn run_restore(flags: Flags) -> Result<()> {
    flags.reject_unknown(
        &["archive", "data-dir", "media-dir", "saves-dir"],
        &["json", "force", "dry-run"],
    )?;
    let archive = flags
        .optional_path("archive")
        .context("--archive is required")?;
    let data_dir = flags.path_or_env("data-dir", "LUDEX_DATA_DIR", "./data")?;
    let media_dir = flags.optional_path("media-dir");
    let saves_dir = flags.optional_path("saves-dir");

    let request = RestoreRequest {
        archive,
        data_dir,
        media_dir,
        saves_dir,
        force: flags.flag("force"),
        dry_run: flags.flag("dry-run"),
    };
    let runtime = tokio::runtime::Runtime::new().context("build tokio runtime")?;
    let report = runtime.block_on(restore_backup(&request))?;

    if flags.flag("json") {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if report.dry_run {
        println!(
            "dry run: archive is valid ({} files, {} bytes, created {}, Ludex {})",
            report.files,
            report.bytes,
            report
                .created_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            report.ludex_version
        );
        println!(
            "components that would be restored: {}",
            report.components.join(", ")
        );
        return Ok(());
    }
    println!(
        "restored {} files ({} bytes) from {} into {}",
        report.files,
        report.bytes,
        report
            .created_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        request.data_dir.display()
    );
    for path in &report.previous {
        println!("previous directory kept at {}", path.display());
    }
    Ok(())
}

fn run_migrate(flags: Flags) -> Result<()> {
    flags.reject_unknown(&["from"], &["json", "delete-source"])?;
    let source = flags.path_or_env("from", "LUDEX_MEDIA_DIR", "./media")?;
    let storage = s3_storage()?;
    let runtime = tokio::runtime::Runtime::new().context("build tokio runtime")?;
    let report = runtime.block_on(migrate_media(
        &source,
        storage.as_ref(),
        flags.flag("delete-source"),
    ))?;
    if flags.flag("json") {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "migrated media: {} uploaded ({} bytes), {} already present, {} source files removed",
            report.uploaded, report.bytes, report.skipped, report.deleted
        );
    }
    Ok(())
}

fn s3_storage() -> Result<std::sync::Arc<dyn ludex_server::storage::StorageBackend>> {
    let config = StorageConfig::from_env()?;
    if config.kind != StorageKind::S3 {
        anyhow::bail!("this command needs LUDEX_STORAGE_BACKEND=s3 and the LUDEX_S3_* settings");
    }
    config.build(&PathBuf::from("."))
}

fn print_usage() {
    println!(
        "ludex-backup {version}\n\
         \n\
         USAGE:\n\
         \x20 ludex-backup backup [--data-dir DIR] [--media-dir DIR] [--saves-dir DIR]\n\
         \x20                     [--out FILE|DIR] [--keep N] [--s3] [--json]\n\
         \x20 ludex-backup restore --archive FILE [--data-dir DIR] [--media-dir DIR]\n\
         \x20                      [--saves-dir DIR] [--force] [--dry-run] [--json]\n\
         \x20 ludex-backup migrate-media [--from DIR] [--delete-source] [--json]\n\
         \n\
         Defaults: LUDEX_DATA_DIR=./data, LUDEX_MEDIA_DIR=./media,\n\
         LUDEX_BACKUP_OUT=./backups, LUDEX_BACKUP_KEEP unset (no retention).\n\
         --s3 uploads with the backend from LUDEX_STORAGE_BACKEND=s3 / LUDEX_S3_*.",
        version = env!("CARGO_PKG_VERSION")
    );
}

/// Minimal `--flag value` / `--flag=value` parser.
struct Flags {
    values: std::collections::HashMap<String, Option<String>>,
}

impl Flags {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self> {
        let mut values = std::collections::HashMap::new();
        let mut arguments = arguments.peekable();
        while let Some(argument) = arguments.next() {
            let Some(name) = argument.strip_prefix("--") else {
                anyhow::bail!("unexpected argument '{argument}'");
            };
            let (name, inline) = match name.split_once('=') {
                Some((name, value)) => (name.to_string(), Some(value.to_string())),
                None => (name.to_string(), None),
            };
            let value = match inline {
                Some(value) => Some(value),
                None => {
                    if matches!(
                        name.as_str(),
                        "s3" | "json" | "force" | "dry-run" | "delete-source" | "help"
                    ) {
                        None
                    } else {
                        Some(
                            arguments
                                .next()
                                .with_context(|| format!("--{name} needs a value"))?,
                        )
                    }
                }
            };
            values.insert(name, value);
        }
        Ok(Self { values })
    }

    fn reject_unknown(&self, values: &[&str], flags: &[&str]) -> Result<()> {
        for name in self.values.keys() {
            anyhow::ensure!(
                values.contains(&name.as_str()) || flags.contains(&name.as_str()),
                "unknown flag --{name}"
            );
        }
        Ok(())
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).and_then(|value| value.as_deref())
    }

    /// Whether a boolean flag was passed.
    fn flag(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    fn optional_path(&self, name: &str) -> Option<PathBuf> {
        self.value(name).map(PathBuf::from)
    }

    fn env_value(&self, name: &str) -> Option<String> {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn path_or_env(&self, name: &str, env: &str, default: &str) -> Result<PathBuf> {
        if let Some(path) = self.optional_path(name) {
            return Ok(path);
        }
        Ok(PathBuf::from(
            self.env_value(env).unwrap_or_else(|| default.to_string()),
        ))
    }
}
