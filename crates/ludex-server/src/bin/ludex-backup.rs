//! `ludex-backup` — consistent backup, restore and verification (#49).
//!
//! ```text
//! ludex-backup backup [--output PATH] [--dry-run] [--no-media] [--upload-s3]
//! ludex-backup restore ARCHIVE [--force] [--dry-run] [--no-media]
//! ludex-backup verify ARCHIVE
//! ```
//!
//! Directories come from the usual `LUDEX_*` variables and can be overridden
//! with `--data-dir`, `--media-dir`, `--games-dir` and `--database-url`.
//!
//! PostgreSQL is not covered: archives hold a SQLite snapshot. Use
//! `pg_dump`/`pg_restore` there and see `docs/POSTGRES.md`.
//! Reports are printed as JSON on stdout; diagnostics go to stderr.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use ludex_server::backup::{
    create_backup, restore_backup, verify_backup, BackupRequest, RestoreRequest,
};
use ludex_server::config::Config;
use ludex_server::storage::ArchiveStorage;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ludex_backup=info".into()),
        )
        .init();
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(());
    }
    let command = args.remove(0);
    let options = Options::parse(args)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    let report: Result<serde_json::Value> = runtime.block_on(async move {
        let config = options.config()?;
        match command.as_str() {
            "backup" => {
                let storage = ArchiveStorage::from_config(&config);
                let request = BackupRequest {
                    data_dir: config.data_dir.clone(),
                    media_dir: config.media_dir.clone(),
                    games_dir: config.games_dir.clone(),
                    database_url: config.database_url.clone(),
                    output: options.output.clone(),
                    dry_run: options.dry_run,
                    include_media: !options.no_media,
                    upload_s3: options.upload_s3,
                    storage: Some(storage),
                    config: Some(config),
                };
                let report = create_backup(&request).await?;
                Ok(serde_json::to_value(report)?)
            }
            "restore" => {
                if ludex_server::db::Backend::from_url(&config.database_url)?
                    == ludex_server::db::Backend::Postgres
                {
                    bail!(
                        "ludex-backup restore only targets SQLite: restore a PostgreSQL dump with \
                         pg_restore (see docs/POSTGRES.md)"
                    );
                }
                let archive = options
                    .archive
                    .clone()
                    .context("restore requires the archive path")?;
                let report = restore_backup(&RestoreRequest {
                    archive,
                    data_dir: config.data_dir.clone(),
                    media_dir: config.media_dir.clone(),
                    force: options.force,
                    dry_run: options.dry_run,
                    include_media: !options.no_media,
                })
                .await?;
                Ok(serde_json::to_value(report)?)
            }
            "verify" => {
                let archive = options
                    .archive
                    .clone()
                    .context("verify requires the archive path")?;
                let manifest = verify_backup(&archive).await?;
                Ok(serde_json::to_value(manifest)?)
            }
            other => bail!("unknown command `{other}`; expected backup, restore or verify"),
        }
    });
    println!("{}", serde_json::to_string_pretty(&report?)?);
    Ok(())
}

#[derive(Debug, Default)]
struct Options {
    archive: Option<PathBuf>,
    output: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    media_dir: Option<PathBuf>,
    games_dir: Option<PathBuf>,
    database_url: Option<String>,
    dry_run: bool,
    force: bool,
    no_media: bool,
    upload_s3: bool,
}

impl Options {
    fn parse(args: Vec<String>) -> Result<Self> {
        let mut options = Self::default();
        let mut iterator = args.into_iter();
        while let Some(argument) = iterator.next() {
            match argument.as_str() {
                "--dry-run" => options.dry_run = true,
                "--force" => options.force = true,
                "--no-media" => options.no_media = true,
                "--upload-s3" => options.upload_s3 = true,
                "--output" | "--data-dir" | "--media-dir" | "--games-dir" | "--database-url" => {
                    let value = iterator
                        .next()
                        .with_context(|| format!("{argument} needs a value"))?;
                    match argument.as_str() {
                        "--output" => options.output = Some(PathBuf::from(value)),
                        "--data-dir" => options.data_dir = Some(PathBuf::from(value)),
                        "--media-dir" => options.media_dir = Some(PathBuf::from(value)),
                        "--games-dir" => options.games_dir = Some(PathBuf::from(value)),
                        _ => options.database_url = Some(value),
                    }
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other if other.starts_with("--") => bail!("unknown option `{other}`"),
                other => {
                    if options.archive.is_some() {
                        bail!("unexpected argument `{other}`");
                    }
                    options.archive = Some(PathBuf::from(other));
                }
            }
        }
        Ok(options)
    }

    fn config(&self) -> Result<Config> {
        let mut config =
            Config::from_env().context("read LUDEX_* configuration (or pass --data-dir)")?;
        if let Some(data_dir) = &self.data_dir {
            config.data_dir = data_dir.clone();
            config.database_url = ludex_server::config::default_database_url(data_dir);
        }
        if let Some(database_url) = &self.database_url {
            config.database_url = database_url.clone();
        }
        if let Some(media_dir) = &self.media_dir {
            config.media_dir = media_dir.clone();
        }
        if let Some(games_dir) = &self.games_dir {
            config.games_dir = games_dir.clone();
        }
        Ok(config)
    }
}

fn print_help() {
    eprintln!(
        "\
ludex-backup — consistent backup and restore for Ludex

USAGE:
  ludex-backup backup [--output PATH] [--dry-run] [--no-media] [--upload-s3]
  ludex-backup restore ARCHIVE [--force] [--dry-run] [--no-media]
  ludex-backup verify ARCHIVE

OPTIONS:
  --data-dir PATH       Override LUDEX_DATA_DIR (also resets the database URL)
  --media-dir PATH      Override LUDEX_MEDIA_DIR
  --games-dir PATH      Override LUDEX_GAMES_DIR (manifest only)
  --database-url URL    Override LUDEX_DATABASE_URL
  --output PATH         Archive destination (default: DATA/backups/ludex-backup-<ts>.tar.gz)
  --dry-run             Validate and report without writing
  --force               Restore over existing data
  --no-media            Skip media/ in the archive or restore
  --upload-s3           Upload the finished archive to the configured S3 bucket

Game archives are not included: they live on the library roots or in S3.
"
    );
}
