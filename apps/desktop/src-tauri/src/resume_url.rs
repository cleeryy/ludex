//! Server base URL for downloads resumed after an app restart (#212 follow-up).
//!
//! Local mode starts the embedded `ludex-server` on an ephemeral port unless
//! `serverPort` is pinned, so the URL a job was recorded against in a previous
//! run stops existing as soon as the app restarts. The persisted parameters of
//! a pending job must therefore follow the live local server, or the resume
//! fails with "connection refused" against a dead port.
//!
//! Remote mode is the opposite: a job recorded against server A must not be
//! resumed against server B just because the user switched the active profile.
//! [`resolve_resume_base`] makes that distinction explicit and testable.

use rusqlite::Connection;

use crate::db;
use crate::settings::Mode;
use crate::state::AppState;

/// True for URLs that point at this machine (`127.0.0.0/8`, `localhost`,
/// `::1`). Only the host is inspected: scheme, port, userinfo and path are
/// irrelevant, and a trailing slash never changes the answer.
pub(crate) fn is_loopback(url: &str) -> bool {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 literal: `[::1]:4321`.
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host == "::1" {
        return true;
    }
    // 127.0.0.0/8: any host in that block is the loopback interface.
    host.strip_prefix("127.")
        .map(|rest| !rest.is_empty() && rest.split('.').all(|octet| octet.parse::<u8>().is_ok()))
        .unwrap_or(false)
}

/// Base URL a resumed job must use, or `None` when the recorded one still
/// applies.
///
/// - Local mode only: the embedded server's URL is what changed, and the local
///   catalog is the same one the job came from.
/// - Only jobs recorded against a loopback URL (or with no URL at all) are
///   rewritten. A job imported from a remote server keeps its origin even when
///   the desktop is now in local mode — its bytes live somewhere else.
/// - An empty `resolved` URL means no local server is up yet; the recorded
///   value is kept so the job can be retried later.
pub(crate) fn resolve_resume_base(recorded: &str, resolved: &str, mode: Mode) -> Option<String> {
    let recorded = recorded.trim().trim_end_matches('/');
    let resolved = resolved.trim().trim_end_matches('/');
    if mode != Mode::Local || resolved.is_empty() || recorded == resolved {
        return None;
    }
    if recorded.is_empty() || is_loopback(recorded) {
        return Some(resolved.to_string());
    }
    None
}

/// Rewrites the recorded server URL of every persisted job parameter set that
/// points at a stale local server. Returns the number of rows updated.
///
/// Runs against a plain connection so the decision rules are testable without a
/// Tauri app handle; [`refresh_pending_install_urls`] adds the state plumbing.
fn refresh_urls(connection: &Connection, resolved: &str, mode: Mode) -> usize {
    let records = match db::all_install_job_params(connection) {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(error = %error, "could not read pending download parameters");
            return 0;
        }
    };
    let mut updated = 0usize;
    for record in records {
        let Some(next) = resolve_resume_base(&record.server_url, resolved, mode) else {
            continue;
        };
        match db::set_install_job_server_url(connection, &record.job_id, &next) {
            Ok(()) => updated += 1,
            Err(error) => tracing::warn!(
                job_id = %record.job_id,
                error = %error,
                "could not refresh a pending download URL"
            ),
        }
    }
    if updated > 0 {
        tracing::info!(updated, url = %resolved, "refreshed pending download URLs for the local server");
    }
    updated
}

/// Called whenever the embedded server reports its URL (startup and restarts),
/// before `install::auto_resume_on_startup` rebuilds the interrupted jobs.
pub(crate) fn refresh_pending_install_urls(state: &AppState, resolved: &str) -> usize {
    let settings = state.settings_snapshot();
    if settings.mode != Mode::Local {
        return 0;
    }
    let connection = match state.db.lock() {
        Ok(connection) => connection,
        Err(_) => {
            tracing::warn!("could not refresh pending download URLs: database unavailable");
            return 0;
        }
    };
    refresh_urls(&connection, resolved, settings.mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludex-resume-url-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn params(job_id: &str, server_url: &str) -> db::InstallJobParamsRecord {
        db::InstallJobParamsRecord {
            job_id: job_id.into(),
            game_id: 1,
            version_id: 1,
            title: "Game".into(),
            version: None,
            executable: None,
            install_dir: None,
            download_dir: None,
            install_location_id: None,
            download_location_id: None,
            expected_bytes: None,
            stream: false,
            archive_format: None,
            replace_path: None,
            server_url: server_url.into(),
            created_at: "2026-11-16T00:00:00Z".into(),
        }
    }

    #[test]
    fn loopback_hosts_are_recognised() {
        for url in [
            "http://127.0.0.1:4321",
            "http://127.0.0.1:4321/",
            "http://localhost:4321",
            "http://LOCALHOST:4321/api",
            "http://[::1]:4321",
            "http://127.0.0.2:1",
            "http://user:pass@127.0.0.1:4321",
        ] {
            assert!(is_loopback(url), "{url} should be loopback");
        }
        for url in [
            "https://ludex.example.com",
            "http://10.1.20.50:8080",
            "http://127.0.0.1.example.com:4321",
            "http://192.168.1.10:4321",
            "http://localhost.example.com",
            "",
        ] {
            assert!(!is_loopback(url), "{url} should not be loopback");
        }
    }

    #[test]
    fn local_port_change_moves_the_resume_url() {
        let next = resolve_resume_base(
            "http://127.0.0.1:41234",
            "http://127.0.0.1:52345",
            Mode::Local,
        );
        assert_eq!(next.as_deref(), Some("http://127.0.0.1:52345"));
    }

    #[test]
    fn unchanged_loopback_url_is_left_alone() {
        assert_eq!(
            resolve_resume_base(
                "http://127.0.0.1:52345/",
                "http://127.0.0.1:52345",
                Mode::Local
            ),
            None
        );
    }

    #[test]
    fn empty_recorded_url_adopts_the_live_server() {
        assert_eq!(
            resolve_resume_base("", "http://127.0.0.1:52345", Mode::Local).as_deref(),
            Some("http://127.0.0.1:52345")
        );
    }

    #[test]
    fn empty_resolved_url_keeps_the_recorded_origin() {
        assert_eq!(
            resolve_resume_base("http://127.0.0.1:41234", "", Mode::Local),
            None
        );
    }

    #[test]
    fn remote_mode_never_rewrites_a_job() {
        // Switching the active profile must not redirect an in-flight download
        // to another catalog, and a remote job keeps its origin.
        assert_eq!(
            resolve_resume_base(
                "https://games.example.com",
                "https://other.example.com",
                Mode::Remote
            ),
            None
        );
        assert_eq!(
            resolve_resume_base(
                "http://127.0.0.1:41234",
                "http://127.0.0.1:52345",
                Mode::Remote
            ),
            None
        );
    }

    #[test]
    fn remote_origin_is_preserved_even_in_local_mode() {
        // The job was recorded against a remote catalog; the bytes live there,
        // not on this machine's embedded server.
        assert_eq!(
            resolve_resume_base(
                "https://games.example.com",
                "http://127.0.0.1:52345",
                Mode::Local
            ),
            None
        );
    }

    #[test]
    fn refresh_updates_the_persisted_rows() {
        let dir = temp_dir("refresh");
        let connection = db::open(&dir.join("index.db")).expect("db");
        db::upsert_install_job_params(&connection, &params("local-job", "http://127.0.0.1:41234"))
            .expect("insert local");
        db::upsert_install_job_params(
            &connection,
            &params("remote-job", "https://games.example.com"),
        )
        .expect("insert remote");
        db::upsert_install_job_params(&connection, &params("pinned-job", "http://127.0.0.1:52345"))
            .expect("insert pinned");

        let updated = refresh_urls(&connection, "http://127.0.0.1:52345", Mode::Local);
        assert_eq!(updated, 1);

        let local = db::get_install_job_params(&connection, "local-job")
            .expect("read local")
            .expect("local row");
        assert_eq!(local.server_url, "http://127.0.0.1:52345");
        let remote = db::get_install_job_params(&connection, "remote-job")
            .expect("read remote")
            .expect("remote row");
        assert_eq!(remote.server_url, "https://games.example.com");

        // Running again is idempotent: nothing is stale any more.
        assert_eq!(
            refresh_urls(&connection, "http://127.0.0.1:52345", Mode::Local),
            0
        );
    }

    #[test]
    fn refresh_is_a_no_op_in_remote_mode() {
        let dir = temp_dir("remote");
        let connection = db::open(&dir.join("index.db")).expect("db");
        db::upsert_install_job_params(&connection, &params("job", "http://127.0.0.1:41234"))
            .expect("insert");

        assert_eq!(
            refresh_urls(&connection, "http://127.0.0.1:52345", Mode::Remote),
            0
        );
        let job = db::get_install_job_params(&connection, "job")
            .expect("read")
            .expect("row");
        assert_eq!(job.server_url, "http://127.0.0.1:41234");
    }
}
