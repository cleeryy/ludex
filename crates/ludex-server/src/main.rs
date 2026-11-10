use std::path::PathBuf;

use anyhow::{bail, Context};
use ludex_server::gamevault::{self, ImportOptions};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ludex_server=info,tower_http=info".into()),
        )
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    runtime.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(command) = args.first().cloned() {
        match command.as_str() {
            "import-gamevault" => {
                args.remove(0);
                return import_gamevault(args).await;
            }
            "--help" | "-h" | "help" => {
                print_help();
                return Ok(());
            }
            other if !other.starts_with('-') => {
                bail!("unknown command '{other}'; run `ludex-server --help`");
            }
            _ => {}
        }
    }

    let config = ludex_server::Config::from_env()?;
    tracing::info!(bind = %config.bind, "starting Ludex server");
    ludex_server::run(config).await
}

fn print_help() {
    println!(
        "Ludex server

Usage:
  ludex-server                         start the HTTP server (LUDEX_* environment)
  ludex-server import-gamevault --dump <file> [--dry-run] [--json]
                                       import a GameVault JSON export (#6)

The import reads the JSON document produced by scripts/gamevault-export.sql.
LUDEX_DATABASE_URL, LUDEX_DATA_DIR, LUDEX_GAMES_DIR and LUDEX_MEDIA_DIR select
the target installation. --dry-run reports what would change and writes nothing.
Temporary passwords for created accounts are printed once."
    );
}

struct ImportArgs {
    dump: PathBuf,
    dry_run: bool,
    json: bool,
}

fn parse_import_args(args: Vec<String>) -> anyhow::Result<ImportArgs> {
    let mut dump = None;
    let mut dry_run = false;
    let mut json = false;
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--dump" => {
                let value = iter.next().context("--dump requires a file path")?;
                dump = Some(PathBuf::from(value));
            }
            "--dry-run" => dry_run = true,
            "--json" => json = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown option '{other}' for import-gamevault"),
        }
    }
    let dump = dump.context("import-gamevault requires --dump <file>")?;
    Ok(ImportArgs {
        dump,
        dry_run,
        json,
    })
}

async fn import_gamevault(args: Vec<String>) -> anyhow::Result<()> {
    let parsed = parse_import_args(args)?;
    let config = ludex_server::Config::from_env()?;
    let outcome = gamevault::run(ImportOptions {
        dump: parsed.dump.clone(),
        dry_run: parsed.dry_run,
        config,
    })
    .await?;

    if parsed.json {
        println!("{}", serde_json::to_string_pretty(&outcome.report)?);
    } else {
        print_report(&outcome.report);
    }

    if !outcome.credentials.is_empty() {
        println!();
        println!("Temporary passwords (shown once, never stored):");
        for credential in &outcome.credentials {
            println!("  {}  {}", credential.username, credential.password);
        }
        println!("Ask each user to change their password after signing in.");
    }

    Ok(())
}

fn print_report(report: &gamevault::ImportReport) {
    let mode = if report.dry_run {
        "DRY RUN — nothing was written"
    } else {
        "imported"
    };
    println!(
        "GameVault import from {} ({}, contract v{}) — {mode}",
        report.dump, report.dump_format, report.dump_version
    );
    println!(
        "Games: {} matched, {} unmatched, {} ignored",
        report.matched, report.unmatched, report.ignored
    );
    for game in &report.games {
        let marker = match game.status {
            gamevault::MatchStatus::Matched => "ok  ",
            gamevault::MatchStatus::Unmatched => "miss",
            gamevault::MatchStatus::Ignored => "skip",
        };
        let method = game
            .method
            .map(|method| format!(" via {}", method.as_str()))
            .unwrap_or_default();
        let detail = game
            .detail
            .as_deref()
            .map(|detail| format!(" — {detail}"))
            .unwrap_or_default();
        println!(
            "  [{marker}] {} (#{}){method}{detail}",
            game.title, game.gamevault_id
        );
    }
    println!(
        "Accounts: {} created, {} already present",
        report.users_created, report.users_matched
    );
    println!(
        "Progress: {}  Bookmarks: {}  News: {}  Metadata: {}",
        report.progress_imported,
        report.bookmarks_imported,
        report.news_imported,
        report.metadata_updated
    );
    if !report.warnings.is_empty() {
        println!("Warnings ({}):", report.warnings.len());
        for warning in &report.warnings {
            println!("  - {warning}");
        }
    }
    if report.dry_run {
        println!("Run again without --dry-run to apply these changes.");
    }
}
