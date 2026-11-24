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
            // Machine-readable contract used by `scripts/gen-types.mjs` and the
            // `Types (generated, up to date)` CI job. No trailing newline: the
            // dump is the exact document, so it can be diffed byte for byte.
            "openapi" => {
                use std::io::Write;
                let document = ludex_server::openapi::document();
                print!("{}", serde_json::to_string_pretty(&document)?);
                std::io::stdout().flush()?;
                return Ok(());
            }
            "healthcheck" => {
                return healthcheck().await;
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
  ludex-server healthcheck             probe GET /api/health on LUDEX_BIND
  ludex-server openapi                 print the OpenAPI document as JSON
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
        // Deliberate one-time credential handoff (#266): the CLI import prints
        // the generated temporary passwords exactly once because there is no
        // other channel to reach the importing administrator. Nothing is
        // logged, persisted or audited, the command documents the behaviour
        // (`--help`), and every account must change the password on first use.
        println!();
        println!("Temporary passwords (shown once, never stored):");
        for credential in &outcome.credentials {
            // Intentional one-time terminal print in the import CLI,
            // documented above and in `main --help`; the credential never
            // reaches a log or an audit event (see
            // `gamevault::tests::credentials_never_debug_their_password`).
            // codeql[rust/cleartext-logging]
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

fn healthcheck_target(bind: &str) -> anyhow::Result<std::net::SocketAddr> {
    let addr: std::net::SocketAddr = bind
        .trim()
        .parse()
        .with_context(|| format!("invalid LUDEX_BIND address '{bind}'"))?;
    let host = match addr.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        }
        other => other,
    };
    Ok(std::net::SocketAddr::new(host, addr.port()))
}

async fn healthcheck() -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let bind = std::env::var("LUDEX_BIND")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| ludex_server::config::DEFAULT_BIND.to_string());
    let target = healthcheck_target(&bind)?;
    let timeout = std::time::Duration::from_secs(4);
    let mut stream = tokio::time::timeout(timeout, TcpStream::connect(target))
        .await
        .context("healthcheck timed out connecting")?
        .with_context(|| format!("healthcheck could not connect to {target}"))?;

    let request =
        format!("GET /api/health HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    tokio::time::timeout(timeout, stream.write_all(request.as_bytes()))
        .await
        .context("healthcheck timed out writing request")?
        .context("healthcheck failed to send request")?;

    let mut response = [0_u8; 64];
    let read = tokio::time::timeout(timeout, stream.read(&mut response))
        .await
        .context("healthcheck timed out reading response")?
        .context("healthcheck failed to read response")?;
    let status_line = std::str::from_utf8(&response[..read])
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("");
    if status_line.starts_with("HTTP/1.1 200") || status_line.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        bail!("unhealthy response from {target}: {status_line}");
    }
}

#[cfg(test)]
mod tests {
    use super::healthcheck_target;

    #[test]
    fn healthcheck_maps_unspecified_bind_addresses_to_loopback() {
        let ipv4 = healthcheck_target("0.0.0.0:8080").expect("ipv4");
        assert_eq!(ipv4.to_string(), "127.0.0.1:8080");
        let ipv6 = healthcheck_target("[::]:9090").expect("ipv6");
        assert_eq!(ipv6.to_string(), "[::1]:9090");
        let explicit = healthcheck_target("127.0.0.1:8081").expect("explicit");
        assert_eq!(explicit.to_string(), "127.0.0.1:8081");
        assert!(healthcheck_target("not-an-address").is_err());
    }
}
