//! Multiple Ludex servers and accounts per installation (#24).
//!
//! Profiles are client-side state: `settings.json` keeps the metadata (label,
//! URL, last use) and the OS keychain or SQLite store keeps the per-profile
//! tokens (see `secrets.rs`). The server database is untouched; each profile
//! gets its own desktop index, so switching servers never mixes installed games,
//! play sessions or download jobs.
//!
//! The existing single-server configuration is migrated on first launch: its
//! URL becomes a profile with a stable id and the active connection keeps
//! working.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::secrets::SecretStore;
use crate::settings::{LocalSettings, Mode};
use crate::util::now_rfc3339;

/// Id of the embedded (local library) server. The port changes across restarts,
/// so the id must not be derived from the URL.
pub const LOCAL_SERVER_ID: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProfile {
    pub id: String,
    pub label: String,
    pub url: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub id: String,
    pub label: String,
    pub url: String,
    pub username: Option<String>,
    pub last_used_at: Option<String>,
    pub created_at: Option<String>,
    pub active: bool,
    pub local: bool,
    pub has_token: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerList {
    pub servers: Vec<ServerView>,
    pub active_id: Option<String>,
    pub token_storage: String,
    pub keychain: Option<String>,
    pub warning: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTest {
    pub ok: bool,
    pub status: Option<u16>,
    pub version: Option<String>,
    pub latency_ms: u64,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoveResult {
    pub was_active: bool,
    pub has_fallback: bool,
}

/// Normalise a server address: add `http://` when the scheme is missing,
/// lowercase the scheme and authority, strip a trailing slash and reject
/// anything that cannot be a plain Ludex origin.
pub fn normalize_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter the Ludex server address.".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("The server address must not contain control characters.".to_string());
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let (scheme, rest) = candidate
        .split_once("://")
        .ok_or_else(|| "The server address is not a valid URL.".to_string())?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "The server address must use http:// or https:// (got {scheme}://)."
        ));
    }
    if rest.is_empty() {
        return Err("The server address is missing a host.".to_string());
    }
    if rest.contains('@') {
        return Err(
            "Server addresses must not contain credentials; save the token separately.".to_string(),
        );
    }
    if rest.contains(char::is_whitespace) {
        return Err("The server address must not contain whitespace.".to_string());
    }

    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, path),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err("The server address is missing a host.".to_string());
    }
    if authority.contains(['?', '#']) || path.contains(['?', '#']) {
        return Err("The server address must not contain a query or a fragment.".to_string());
    }
    if authority.starts_with(':') || authority.ends_with(':') {
        return Err("The server address has an invalid host or port.".to_string());
    }
    let authority = authority.to_ascii_lowercase();
    let path = path.trim_matches('/');
    Ok(if path.is_empty() {
        format!("{scheme}://{authority}")
    } else {
        format!("{scheme}://{authority}/{path}")
    })
}

/// Stable profile id derived from the normalised URL.
pub fn server_id_for(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    format!("srv_{}", hex::encode(&digest[..8]))
}

fn host_label(url: &str) -> String {
    url.split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

fn default_label(id: &str, url: &str) -> String {
    if id == LOCAL_SERVER_ID {
        "This device".to_string()
    } else {
        host_label(url)
    }
}

/// Key used in the secret store.
pub fn secret_key(profile_id: &str) -> String {
    format!("server:{profile_id}")
}

/// Id the current connection should use: `local` for the embedded server, the
/// URL hash otherwise.
fn current_id(settings: &LocalSettings) -> String {
    if settings.mode == Mode::Local {
        LOCAL_SERVER_ID.to_string()
    } else {
        server_id_for(settings.server_url.trim_end_matches('/'))
    }
}

/// Create or refresh the profile that matches `settings.serverUrl`.
///
/// Idempotent; it never reorders profiles and never overwrites a label.
pub fn sync_profiles(settings: &mut LocalSettings, now: &str) {
    let url = settings.server_url.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return;
    }
    let id = current_id(settings);

    if let Some(profile) = settings.servers.iter_mut().find(|profile| profile.id == id) {
        profile.url = url;
        if profile.label.trim().is_empty() {
            profile.label = default_label(&id, &profile.url);
        }
        settings.active_server_id = id;
        return;
    }

    if let Some(profile) = settings
        .servers
        .iter_mut()
        .find(|profile| profile.url == url)
    {
        settings.active_server_id = profile.id.clone();
        return;
    }

    settings.servers.push(ServerProfile {
        id: id.clone(),
        label: default_label(&id, &url),
        url,
        username: None,
        last_used_at: Some(now.to_string()),
        created_at: Some(now.to_string()),
    });
    settings.active_server_id = id;
}

/// Add a profile or update an existing one. Remote ids follow the URL, so
/// editing the address of an existing server creates a new identity.
pub fn upsert(
    settings: &mut LocalSettings,
    existing_id: Option<&str>,
    label: &str,
    url: &str,
    username: Option<&str>,
) -> Result<ServerProfile, String> {
    let url = normalize_url(url)?;
    let existing = existing_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(|id| {
            settings
                .servers
                .iter()
                .find(|profile| profile.id == id)
                .cloned()
        });
    let original_id = existing.as_ref().map(|profile| profile.id.clone());

    let id = if existing
        .as_ref()
        .map(|profile| profile.id == LOCAL_SERVER_ID)
        .unwrap_or(false)
    {
        LOCAL_SERVER_ID.to_string()
    } else {
        server_id_for(&url)
    };

    if settings
        .servers
        .iter()
        .any(|profile| profile.id == id && Some(&profile.id) != original_id.as_ref())
    {
        return Err("That server is already saved.".to_string());
    }
    if settings
        .servers
        .iter()
        .any(|profile| profile.url == url && profile.id != id)
    {
        return Err("That server is already saved.".to_string());
    }

    let label = if label.trim().is_empty() {
        default_label(&id, &url)
    } else {
        label.trim().chars().take(60).collect::<String>()
    };
    let username = username
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let profile = match existing {
        Some(mut profile) => {
            let was_active = settings.active_server_id == profile.id;
            let url_changed = profile.url != url;
            profile.id = id.clone();
            profile.label = label;
            profile.url = url.clone();
            if username.is_some() {
                profile.username = username;
            }
            if was_active {
                settings.active_server_id = id.clone();
                settings.server_url = url.clone();
                settings.mode = if id == LOCAL_SERVER_ID {
                    Mode::Local
                } else {
                    Mode::Remote
                };
                // A different address means a different session; never send
                // the old server's token to the new one.
                if url_changed {
                    settings.token = None;
                }
            }
            if let Some(slot) = settings
                .servers
                .iter_mut()
                .find(|candidate| Some(&candidate.id) == original_id.as_ref())
            {
                *slot = profile.clone();
            }
            profile
        }
        None => {
            let profile = ServerProfile {
                id: id.clone(),
                label,
                url: url.clone(),
                username,
                last_used_at: None,
                created_at: Some(now_rfc3339()),
            };
            settings.servers.push(profile.clone());
            profile
        }
    };

    Ok(profile)
}

/// Point the app at a saved profile. The caller stores/loads the token.
pub fn apply_active(
    settings: &mut LocalSettings,
    id: &str,
    token: Option<String>,
    now: &str,
) -> Result<ServerProfile, String> {
    let profile = settings
        .servers
        .iter_mut()
        .find(|profile| profile.id == id)
        .cloned()
        .ok_or_else(|| "That server is not saved.".to_string())?;

    settings.mode = if profile.id == LOCAL_SERVER_ID {
        Mode::Local
    } else {
        Mode::Remote
    };
    settings.server_url = profile.url.clone();
    settings.token = token;
    settings.active_server_id = profile.id.clone();
    if let Some(stored) = settings
        .servers
        .iter_mut()
        .find(|candidate| candidate.id == profile.id)
    {
        stored.last_used_at = Some(now.to_string());
    }
    Ok(profile)
}

/// Remove a saved profile. When it was active, the first remaining profile
/// becomes active; with none left the connection is cleared.
pub fn remove(settings: &mut LocalSettings, id: &str) -> Result<RemoveResult, String> {
    let index = settings
        .servers
        .iter()
        .position(|profile| profile.id == id)
        .ok_or_else(|| "That server is not saved.".to_string())?;
    let was_active = settings.active_server_id == id;
    settings.servers.remove(index);

    if !was_active {
        return Ok(RemoveResult {
            was_active: false,
            has_fallback: false,
        });
    }

    match settings.servers.first().cloned() {
        Some(next) => {
            settings.active_server_id = next.id.clone();
            settings.server_url = next.url;
            settings.mode = if next.id == LOCAL_SERVER_ID {
                Mode::Local
            } else {
                Mode::Remote
            };
            settings.token = None;
            Ok(RemoveResult {
                was_active: true,
                has_fallback: true,
            })
        }
        None => {
            settings.active_server_id.clear();
            settings.server_url.clear();
            settings.token = None;
            Ok(RemoveResult {
                was_active: true,
                has_fallback: false,
            })
        }
    }
}

/// Settings payload for the servers panel and the shell switcher.
pub fn list(settings: &LocalSettings, store: &SecretStore, message: Option<String>) -> ServerList {
    let active = if settings.active_server_id.trim().is_empty() {
        None
    } else {
        Some(settings.active_server_id.clone())
    };
    let servers = settings
        .servers
        .iter()
        .map(|profile| ServerView {
            id: profile.id.clone(),
            label: profile.label.clone(),
            url: profile.url.clone(),
            username: profile.username.clone(),
            last_used_at: profile.last_used_at.clone(),
            created_at: profile.created_at.clone(),
            active: active.as_deref() == Some(profile.id.as_str()),
            local: profile.id == LOCAL_SERVER_ID,
            has_token: store.get(&secret_key(&profile.id)).ok().flatten().is_some(),
        })
        .collect();

    ServerList {
        servers,
        active_id: active,
        token_storage: store.backend().as_str().to_string(),
        keychain: store.keychain_name().map(str::to_string),
        warning: store.warning(),
        message,
    }
}

/// Desktop index for one profile. `active_id` empty keeps the legacy path for
/// an installation with no server configured yet.
pub fn database_path(app_data: &Path, active_id: &str) -> PathBuf {
    if active_id.trim().is_empty() {
        return app_data.join("ludex-desktop.db");
    }
    let component = crate::paths::sanitize_component(active_id, "server");
    app_data.join("servers").join(component).join("index.db")
}

/// Path for the profile database, copying the legacy single-server index the
/// first time a profile is used. The legacy file is kept as a backup.
pub fn adopt_legacy_database(app_data: &Path, active_id: &str) -> Result<PathBuf, String> {
    let target = database_path(app_data, active_id);
    if active_id.trim().is_empty() || target.is_file() {
        return Ok(target);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create the desktop data folder {}: {error}",
                parent.display()
            )
        })?;
    }

    let legacy = app_data.join("ludex-desktop.db");
    if legacy.is_file() {
        std::fs::copy(&legacy, &target).map_err(|error| {
            format!("Could not move the desktop index into the new server profile: {error}")
        })?;
        // WAL sidecars belong to the copy as well.
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", legacy.display()));
            if sidecar.is_file() {
                let destination = PathBuf::from(format!("{}{suffix}", target.display()));
                let _ = std::fs::copy(&sidecar, destination);
            }
        }
        tracing::info!(
            from = %legacy.display(),
            to = %target.display(),
            "adopted the existing desktop database into the active server profile"
        );
    }
    Ok(target)
}

/// `GET {url}/api/health` with a short timeout. Never panics; failures are a
/// structured result so the dialog can show the real reason.
pub async fn test_connection(http: &reqwest::Client, url: &str, token: Option<&str>) -> ServerTest {
    let normalized = match normalize_url(url) {
        Ok(url) => url,
        Err(message) => {
            return ServerTest {
                ok: false,
                status: None,
                version: None,
                latency_ms: 0,
                message,
            };
        }
    };

    let started = Instant::now();
    let mut request = http
        .get(format!("{normalized}/api/health"))
        .timeout(Duration::from_secs(8));
    if let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) {
        request = request.bearer_auth(token);
    }

    match request.send().await {
        Ok(response) => {
            let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let status = response.status();
            let payload: Option<serde_json::Value> = response.json().await.ok();
            let version = payload
                .as_ref()
                .and_then(|value| value.get("version"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            if status.is_success() {
                ServerTest {
                    ok: true,
                    status: Some(status.as_u16()),
                    version: version.clone(),
                    latency_ms,
                    message: match version {
                        Some(version) => {
                            format!("Connected. Ludex {version} answered in {latency_ms} ms.")
                        }
                        None => format!("Connected in {latency_ms} ms."),
                    },
                }
            } else {
                ServerTest {
                    ok: false,
                    status: Some(status.as_u16()),
                    version,
                    latency_ms,
                    message: format!(
                        "The server answered with HTTP {}. Check the address and the token.",
                        status.as_u16()
                    ),
                }
            }
        }
        Err(error) => {
            let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            ServerTest {
                ok: false,
                status: None,
                version: None,
                latency_ms,
                message: format!("Could not reach {normalized}: {error}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-profiles-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn urls_are_normalized_and_validated() {
        assert_eq!(
            normalize_url("  HTTPS://Ludex.Example.com/  ").unwrap(),
            "https://ludex.example.com"
        );
        assert_eq!(
            normalize_url("ludex.example.com:8443").unwrap(),
            "http://ludex.example.com:8443"
        );
        assert_eq!(
            normalize_url("http://example.com/base/path/").unwrap(),
            "http://example.com/base/path"
        );
        assert!(normalize_url("ftp://example.com").is_err());
        assert!(normalize_url("http://user:pass@example.com").is_err());
        assert!(normalize_url("http://exa mple.com").is_err());
        assert!(normalize_url("http://example.com/?a=1").is_err());
        assert!(normalize_url("http://example.com/#frag").is_err());
        assert!(normalize_url("   ").is_err());
        assert!(normalize_url("http://:8080").is_err());
    }

    #[test]
    fn ids_are_stable_and_url_scoped() {
        let one = server_id_for("http://example.com");
        let two = server_id_for("http://example.com");
        let other = server_id_for("http://example.com:8080");
        assert_eq!(one, two);
        assert_ne!(one, other);
        assert!(one.starts_with("srv_"));
        assert_eq!(one.len(), 20);
    }

    #[test]
    fn migration_creates_one_profile_and_is_idempotent() {
        let mut settings = LocalSettings {
            mode: Mode::Remote,
            server_url: "https://ludex.example.com/".to_string(),
            token: Some("jwt".to_string()),
            ..LocalSettings::default()
        };
        sync_profiles(&mut settings, "2026-11-11T10:00:00Z");
        assert_eq!(settings.servers.len(), 1);
        assert_eq!(settings.servers[0].url, "https://ludex.example.com");
        assert_eq!(settings.servers[0].label, "ludex.example.com");
        assert_eq!(
            settings.active_server_id,
            server_id_for("https://ludex.example.com")
        );
        // The token stays on the active connection, never in the profile list.
        assert_eq!(settings.token.as_deref(), Some("jwt"));

        let snapshot = settings.servers.clone();
        sync_profiles(&mut settings, "2026-11-11T10:05:00Z");
        assert_eq!(settings.servers, snapshot);
    }

    #[test]
    fn local_profile_keeps_a_stable_id_when_the_port_changes() {
        let mut settings = LocalSettings {
            mode: Mode::Local,
            server_url: "http://127.0.0.1:41000".to_string(),
            ..LocalSettings::default()
        };
        sync_profiles(&mut settings, "2026-11-11T10:00:00Z");
        assert_eq!(settings.servers.len(), 1);
        assert_eq!(settings.servers[0].id, LOCAL_SERVER_ID);
        assert_eq!(settings.servers[0].label, "This device");

        settings.server_url = "http://127.0.0.1:42001".to_string();
        sync_profiles(&mut settings, "2026-11-11T10:05:00Z");
        assert_eq!(settings.servers.len(), 1);
        assert_eq!(settings.servers[0].url, "http://127.0.0.1:42001");
    }

    #[test]
    fn upsert_rejects_duplicate_urls_and_defaults_the_label() {
        let mut settings = LocalSettings::default();
        let profile = upsert(
            &mut settings,
            None,
            "  ",
            "https://one.example.com",
            Some("analyste"),
        )
        .unwrap();
        assert_eq!(profile.label, "one.example.com");
        assert_eq!(profile.username.as_deref(), Some("analyste"));
        assert_eq!(settings.servers.len(), 1);

        let error = upsert(
            &mut settings,
            None,
            "Dupe",
            "https://one.example.com/",
            None,
        )
        .unwrap_err();
        assert!(error.contains("already saved"));
    }

    #[test]
    fn editing_the_active_address_updates_the_connection() {
        let mut settings = LocalSettings {
            mode: Mode::Remote,
            servers: vec![ServerProfile {
                id: server_id_for("https://one.example.com"),
                label: "One".to_string(),
                url: "https://one.example.com".to_string(),
                username: None,
                last_used_at: None,
                created_at: None,
            }],
            active_server_id: server_id_for("https://one.example.com"),
            server_url: "https://one.example.com".to_string(),
            token: Some("jwt".to_string()),
            ..LocalSettings::default()
        };
        let edited = upsert(
            &mut settings,
            Some(&server_id_for("https://one.example.com")),
            "One",
            "https://two.example.com",
            None,
        )
        .unwrap();
        assert_eq!(edited.id, server_id_for("https://two.example.com"));
        assert_eq!(settings.servers.len(), 1);
        assert_eq!(settings.server_url, "https://two.example.com");
        assert_eq!(settings.active_server_id, edited.id);
        // The old server's session must not travel to the new address.
        assert!(settings.token.is_none());
    }

    #[test]
    fn renaming_the_active_profile_keeps_its_token() {
        let id = server_id_for("https://one.example.com");
        let mut settings = LocalSettings {
            mode: Mode::Remote,
            servers: vec![ServerProfile {
                id: id.clone(),
                label: "One".to_string(),
                url: "https://one.example.com".to_string(),
                username: None,
                last_used_at: None,
                created_at: None,
            }],
            active_server_id: id.clone(),
            server_url: "https://one.example.com".to_string(),
            token: Some("jwt".to_string()),
            ..LocalSettings::default()
        };
        upsert(
            &mut settings,
            Some(&id),
            "Living room",
            "https://one.example.com",
            None,
        )
        .unwrap();
        assert_eq!(settings.servers[0].label, "Living room");
        assert_eq!(settings.token.as_deref(), Some("jwt"));
    }

    #[test]
    fn removing_the_active_profile_falls_back_to_the_first_one() {
        let mut settings = LocalSettings {
            servers: vec![
                ServerProfile {
                    id: "a".to_string(),
                    label: "A".to_string(),
                    url: "https://a.example.com".to_string(),
                    username: None,
                    last_used_at: None,
                    created_at: None,
                },
                ServerProfile {
                    id: "b".to_string(),
                    label: "B".to_string(),
                    url: "https://b.example.com".to_string(),
                    username: None,
                    last_used_at: None,
                    created_at: None,
                },
            ],
            active_server_id: "b".to_string(),
            mode: Mode::Remote,
            server_url: "https://b.example.com".to_string(),
            token: Some("jwt".to_string()),
            ..LocalSettings::default()
        };
        let result = remove(&mut settings, "b").unwrap();
        assert!(result.was_active && result.has_fallback);
        assert_eq!(settings.server_url, "https://a.example.com");
        assert_eq!(settings.active_server_id, "a");
        assert!(settings.token.is_none());

        let result = remove(&mut settings, "a").unwrap();
        assert!(result.was_active && !result.has_fallback);
        assert!(settings.server_url.is_empty());
        assert!(settings.active_server_id.is_empty());
    }

    #[test]
    fn removing_an_inactive_profile_keeps_the_connection() {
        let mut settings = LocalSettings {
            servers: vec![
                ServerProfile {
                    id: "a".to_string(),
                    label: "A".to_string(),
                    url: "https://a.example.com".to_string(),
                    username: None,
                    last_used_at: None,
                    created_at: None,
                },
                ServerProfile {
                    id: "b".to_string(),
                    label: "B".to_string(),
                    url: "https://b.example.com".to_string(),
                    username: None,
                    last_used_at: None,
                    created_at: None,
                },
            ],
            active_server_id: "a".to_string(),
            server_url: "https://a.example.com".to_string(),
            ..LocalSettings::default()
        };
        let result = remove(&mut settings, "b").unwrap();
        assert!(!result.was_active);
        assert_eq!(settings.server_url, "https://a.example.com");
        assert_eq!(settings.servers.len(), 1);
    }

    #[test]
    fn database_paths_are_isolated_per_profile() {
        let data = Path::new("/data");
        assert_eq!(
            database_path(data, "local"),
            PathBuf::from("/data/servers/local/index.db")
        );
        assert_eq!(
            database_path(data, "srv_abcdef0123456789"),
            PathBuf::from("/data/servers/srv_abcdef0123456789/index.db")
        );
        assert_eq!(
            database_path(data, "srv_other"),
            PathBuf::from("/data/servers/srv_other/index.db")
        );
        assert_eq!(
            database_path(data, ""),
            PathBuf::from("/data/ludex-desktop.db")
        );
        assert_ne!(database_path(data, "srv_a"), database_path(data, "srv_b"));
    }

    #[test]
    fn legacy_database_is_copied_once_and_kept() {
        let data = temp_dir("legacy");
        std::fs::write(data.join("ludex-desktop.db"), b"legacy-bytes").unwrap();

        let target = adopt_legacy_database(&data, "srv_one").unwrap();
        assert_eq!(
            target,
            data.join("servers").join("srv_one").join("index.db")
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"legacy-bytes");
        assert!(data.join("ludex-desktop.db").is_file());

        std::fs::write(&target, b"profile-bytes").unwrap();
        let again = adopt_legacy_database(&data, "srv_one").unwrap();
        assert_eq!(again, target);
        assert_eq!(std::fs::read(&again).unwrap(), b"profile-bytes");

        let fresh = adopt_legacy_database(&data, "srv_two").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"legacy-bytes");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn list_marks_the_active_profile() {
        let dir = temp_dir("list");
        let store = SecretStore::for_tests(&dir, None);
        let mut settings = LocalSettings::default();
        let profile = upsert(&mut settings, None, "One", "https://one.example.com", None).unwrap();
        settings.mode = Mode::Remote;
        settings.server_url = profile.url.clone();
        settings.active_server_id = profile.id.clone();
        let view = list(&settings, &store, None);
        assert_eq!(view.servers.len(), 1);
        assert!(view.servers[0].active);
        assert_eq!(view.active_id.as_deref(), Some(view.servers[0].id.as_str()));
        assert_eq!(view.token_storage, "database");
        assert!(view.warning.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn connection_test_reports_a_healthy_server() {
        let body = br#"{"status":"ok","version":"9.9.9","uptimeSeconds":1}"#.to_vec();
        let server = crate::test_support::serve_bytes(body).await;
        let http = reqwest::Client::new();
        let result = test_connection(&http, &server.url, None).await;
        assert!(result.ok, "{}", result.message);
        assert_eq!(result.version.as_deref(), Some("9.9.9"));
        assert_eq!(result.status, Some(200));
    }

    #[tokio::test]
    async fn connection_test_reports_an_unreachable_server() {
        let http = reqwest::Client::new();
        let result = test_connection(&http, "http://127.0.0.1:1", None).await;
        assert!(!result.ok);
        assert!(result.message.starts_with("Could not reach"));
    }

    #[tokio::test]
    async fn invalid_urls_fail_the_connection_test_without_network() {
        let result = test_connection(&reqwest::Client::new(), "not a url", None).await;
        assert!(!result.ok);
        assert!(result.status.is_none());
    }
}
