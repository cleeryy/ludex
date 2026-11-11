//! Secret storage for saved server profiles (#24).
//!
//! Preferred backend: the OS keychain. LibreSecret (`secret-tool`) on Linux and
//! the `security` CLI on macOS. Windows has no scriptable read API for the
//! Credential Manager, so it uses the fallback.
//!
//! Fallback: a local SQLite file (`{appDataDir}/secrets.db`, mode 0600). The
//! settings UI shows a warning while this backend is active. The active
//! connection token also stays in `settings.json` (0600) exactly as before this
//! feature, because every desktop command reads it synchronously; the profile
//! store is what keeps the *other* servers' tokens out of the app data folder.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::util::now_rfc3339;

/// Where a saved token lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretBackend {
    Keychain,
    Database,
}

impl SecretBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            SecretBackend::Keychain => "keychain",
            SecretBackend::Database => "database",
        }
    }
}

/// The keychain implementation in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum KeychainKind {
    SecretService,
    MacSecurity,
}

impl KeychainKind {
    pub fn name(self) -> &'static str {
        match self {
            KeychainKind::SecretService => "secret-service",
            KeychainKind::MacSecurity => "macos-keychain",
        }
    }
}

const KEYCHAIN_SERVICE: &str = "cloud.cleeryy.ludex";
const KEYCHAIN_LABEL: &str = "Ludex server token";
const MAX_KEY_CHARS: usize = 120;

#[cfg(target_os = "linux")]
fn detect_keychain() -> Option<KeychainKind> {
    if !forced_backend().eq_ignore_ascii_case("file")
        && crate::paths::find_on_path("secret-tool").is_some()
    {
        return Some(KeychainKind::SecretService);
    }
    None
}

#[cfg(target_os = "macos")]
fn detect_keychain() -> Option<KeychainKind> {
    if !forced_backend().eq_ignore_ascii_case("file")
        && crate::paths::find_on_path("security").is_some()
    {
        return Some(KeychainKind::MacSecurity);
    }
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_keychain() -> Option<KeychainKind> {
    None
}

fn forced_backend() -> String {
    std::env::var("LUDEX_SECRET_BACKEND").unwrap_or_default()
}

/// Validate a secret key: it becomes a keychain attribute and a SQLite primary
/// key, so keep it to a conservative alphabet.
pub fn validate_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.len() > MAX_KEY_CHARS {
        return Err("The secret key is empty or too long.".to_string());
    }
    if !key.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | ':')
    }) {
        return Err(format!(
            "The secret key contains unsupported characters: {key}"
        ));
    }
    Ok(())
}

/// Token vault for saved profiles.
pub struct SecretStore {
    root: PathBuf,
    /// `Some(kind)` while the keychain is usable. Demoted to `None` on the
    /// first failure so a locked or missing keychain does not loop.
    keychain: Mutex<Option<KeychainKind>>,
}

impl SecretStore {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            keychain: Mutex::new(detect_keychain()),
        }
    }

    #[cfg(test)]
    pub fn for_tests(root: &Path, keychain: Option<KeychainKind>) -> Self {
        Self {
            root: root.to_path_buf(),
            keychain: Mutex::new(keychain),
        }
    }

    fn keychain_kind(&self) -> Option<KeychainKind> {
        self.keychain.lock().ok().and_then(|guard| *guard)
    }

    pub fn backend(&self) -> SecretBackend {
        if self.keychain_kind().is_some() {
            SecretBackend::Keychain
        } else {
            SecretBackend::Database
        }
    }

    pub fn keychain_name(&self) -> Option<&'static str> {
        self.keychain_kind().map(KeychainKind::name)
    }

    /// Warning shown in the settings panel while the fallback is active.
    pub fn warning(&self) -> Option<String> {
        (self.backend() == SecretBackend::Database).then(|| {
            "Saved server tokens are stored in a local SQLite file (mode 0600) because no OS keychain is available. Any process running as your user can read it.".to_string()
        })
    }

    fn demote(&self) {
        if let Ok(mut guard) = self.keychain.lock() {
            if guard.is_some() {
                tracing::warn!(
                    "the OS keychain is unavailable; saved token storage falls back to SQLite"
                );
                *guard = None;
            }
        }
    }

    fn database_path(&self) -> PathBuf {
        self.root.join("secrets.db")
    }

    fn database(&self) -> Result<Connection, String> {
        let path = self.database_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create the secret store folder: {error}"))?;
        }
        let connection = Connection::open(&path)
            .map_err(|error| format!("Could not open the secret store: {error}"))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| format!("Could not configure the secret store: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;
                 CREATE TABLE IF NOT EXISTS secrets (
                     key        TEXT PRIMARY KEY,
                     value      TEXT NOT NULL,
                     updated_at TEXT NOT NULL
                 );",
            )
            .map_err(|error| format!("Could not initialise the secret store: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(connection)
    }

    pub fn set(&self, key: &str, value: &str) -> Result<SecretBackend, String> {
        validate_key(key)?;
        if value.is_empty() {
            return Err("Refusing to save an empty token.".to_string());
        }
        if let Some(kind) = self.keychain_kind() {
            match keychain_set(kind, key, value) {
                Ok(()) => return Ok(SecretBackend::Keychain),
                Err(error) => {
                    tracing::warn!(%error, "keychain write failed; falling back to SQLite");
                    self.demote();
                }
            }
        }
        let connection = self.database()?;
        connection
            .execute(
                "INSERT INTO secrets (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key.trim(), value, now_rfc3339()],
            )
            .map_err(|error| format!("Could not save the token: {error}"))?;
        Ok(SecretBackend::Database)
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        validate_key(key)?;
        if let Some(kind) = self.keychain_kind() {
            match keychain_get(kind, key) {
                Ok(value) => return Ok(value),
                Err(error) => {
                    tracing::warn!(%error, "keychain read failed; falling back to SQLite");
                    self.demote();
                }
            }
        }
        let connection = self.database()?;
        connection
            .query_row(
                "SELECT value FROM secrets WHERE key = ?1",
                params![key.trim()],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(format!("Could not read the token: {other}")),
            })
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        validate_key(key)?;
        if let Some(kind) = self.keychain_kind() {
            if let Err(error) = keychain_delete(kind, key) {
                tracing::warn!(%error, "keychain delete failed; clearing the SQLite copy");
                self.demote();
            }
        }
        let connection = self.database()?;
        connection
            .execute("DELETE FROM secrets WHERE key = ?1", params![key.trim()])
            .map_err(|error| format!("Could not remove the token: {error}"))?;
        Ok(())
    }
}

fn keychain_set(kind: KeychainKind, key: &str, value: &str) -> Result<(), String> {
    match kind {
        KeychainKind::SecretService => {
            let mut child = Command::new("secret-tool")
                .args([
                    "store",
                    "--label",
                    KEYCHAIN_LABEL,
                    "service",
                    KEYCHAIN_SERVICE,
                    "key",
                    key,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| format!("secret-tool is not available: {error}"))?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(value.as_bytes())
                    .map_err(|error| format!("Could not write the secret: {error}"))?;
            }
            let output = child
                .wait_with_output()
                .map_err(|error| format!("secret-tool failed: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "secret-tool exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(())
        }
        KeychainKind::MacSecurity => {
            let output = Command::new("security")
                .args([
                    "add-generic-password",
                    "-U",
                    "-s",
                    KEYCHAIN_SERVICE,
                    "-a",
                    key,
                    "-w",
                    value,
                ])
                .stdin(Stdio::null())
                .output()
                .map_err(|error| format!("security is not available: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "security exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(())
        }
    }
}

fn keychain_get(kind: KeychainKind, key: &str) -> Result<Option<String>, String> {
    let output = match kind {
        KeychainKind::SecretService => Command::new("secret-tool")
            .args(["lookup", "service", KEYCHAIN_SERVICE, "key", key])
            .stdin(Stdio::null())
            .output(),
        KeychainKind::MacSecurity => Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                key,
                "-w",
            ])
            .stdin(Stdio::null())
            .output(),
    }
    .map_err(|error| format!("The keychain tool is not available: {error}"))?;

    if !output.status.success() {
        // A missing entry is not an error.
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

fn keychain_delete(kind: KeychainKind, key: &str) -> Result<(), String> {
    let output = match kind {
        KeychainKind::SecretService => Command::new("secret-tool")
            .args(["clear", "service", KEYCHAIN_SERVICE, "key", key])
            .stdin(Stdio::null())
            .output(),
        KeychainKind::MacSecurity => Command::new("security")
            .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", key])
            .stdin(Stdio::null())
            .output(),
    }
    .map_err(|error| format!("The keychain tool is not available: {error}"))?;
    // Deleting a missing entry is a success.
    let _ = output;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-secrets-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn database_roundtrip_and_overwrite() {
        let dir = temp_dir("roundtrip");
        let store = SecretStore::for_tests(&dir, None);
        assert_eq!(store.backend(), SecretBackend::Database);
        assert!(store.warning().is_some());

        store.set("server:srv_1", "token-one").unwrap();
        assert_eq!(
            store.get("server:srv_1").unwrap().as_deref(),
            Some("token-one")
        );

        store.set("server:srv_1", "token-two").unwrap();
        assert_eq!(
            store.get("server:srv_1").unwrap().as_deref(),
            Some("token-two")
        );

        store.delete("server:srv_1").unwrap();
        assert_eq!(store.get("server:srv_1").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_tokens_are_refused() {
        let dir = temp_dir("empty");
        let store = SecretStore::for_tests(&dir, None);
        assert!(store.set("server:x", "").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_are_validated() {
        assert!(validate_key("server:srv_abc").is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key("server/abc").is_err());
        assert!(validate_key("server abc").is_err());
        assert!(validate_key(&"a".repeat(MAX_KEY_CHARS + 1)).is_err());
    }

    #[test]
    fn database_file_is_not_world_readable_on_unix() {
        let dir = temp_dir("perms");
        let store = SecretStore::for_tests(&dir, None);
        store.set("server:x", "secret").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("secrets.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode & 0o077,
                0,
                "secrets.db must not be group/world readable"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
