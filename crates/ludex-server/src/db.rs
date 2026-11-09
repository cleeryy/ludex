//! Database connection, migrations and small shared helpers.

use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

/// Opens the SQLite database and applies migrations.
///
/// `sqlite://` URLs are converted to a filesystem path manually instead of
/// relying on sqlx's URL parser. This keeps Windows data directories such as
/// `C:\Users\Alice\AppData\Roaming\Ludex` (backslashes and spaces) and
/// relative paths working. Other URL forms are still handled by sqlx.
pub async fn connect(database_url: &str) -> Result<SqlitePool> {
    let options = connect_options(database_url)
        .with_context(|| format!("invalid database URL: {database_url}"))?;
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
        .context("connect to SQLite database")?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("apply database migrations")?;

    Ok(pool)
}

fn connect_options(database_url: &str) -> Result<SqliteConnectOptions> {
    let options = match sqlite_path_from_url(database_url) {
        Some(path) => SqliteConnectOptions::new().filename(path),
        None => SqliteConnectOptions::from_str(database_url)?,
    };
    Ok(options
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5)))
}

/// Extracts the filesystem path from a `sqlite://` URL, ignoring query options.
#[must_use]
pub fn sqlite_path_from_url(database_url: &str) -> Option<PathBuf> {
    let rest = database_url.strip_prefix("sqlite://")?;
    let path = rest.split_once('?').map_or(rest, |(path, _)| path);
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(percent_decode(path)))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Current UTC timestamp.
#[must_use]
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Formats a timestamp for storage as RFC 3339 UTC text.
#[must_use]
pub fn db_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parses a timestamp read from SQLite.
#[must_use]
pub fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

/// Parses a user- or provider-supplied date (`YYYY-MM-DD` or RFC 3339).
#[must_use]
pub fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(parsed) = parse_time(trimmed) {
        return Some(parsed);
    }
    if trimmed.len() == 4 {
        if let Ok(year) = trimmed.parse::<i32>() {
            if (1900..=2100).contains(&year) {
                return Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single();
            }
        }
    }
    chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|naive| Utc.from_utc_datetime(&naive))
}

/// Builds a midnight-UTC date from a release year.
#[must_use]
pub fn date_from_year(year: u16) -> Option<DateTime<Utc>> {
    Utc.with_ymd_and_hms(i32::from(year), 1, 1, 0, 0, 0)
        .single()
}

/// Serialises a list of strings for a `*_json` column.
#[must_use]
pub fn dump_json_list(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

/// Reads a `*_json` column holding a list of strings, tolerating old/bad data.
#[must_use]
pub fn parse_json_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// Reads a `settings` value.
pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let value = sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .context("read setting")?;
    Ok(value)
}

/// Writes a `settings` value, replacing any previous value.
pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .context("write setting")?;
    Ok(())
}

/// Number of registered accounts.
pub async fn count_users(pool: &SqlitePool) -> Result<i64> {
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .context("count users")?;
    Ok(count)
}

/// Server display name: the value saved at setup, or the configured default.
pub async fn server_name(pool: &SqlitePool, fallback: &str) -> String {
    match get_setting(pool, "server_name").await {
        Ok(Some(value)) if !value.trim().is_empty() => value,
        _ => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_urls_become_paths_without_query() {
        let path =
            sqlite_path_from_url("sqlite:///srv/ludex/data/ludex.db?mode=rwc").expect("path");
        assert_eq!(path, PathBuf::from("/srv/ludex/data/ludex.db"));

        let path = sqlite_path_from_url("sqlite://./data/ludex.db?mode=rwc").expect("path");
        assert_eq!(path, PathBuf::from("./data/ludex.db"));
    }

    #[test]
    fn windows_style_sqlite_urls_are_preserved() {
        let path = sqlite_path_from_url(
            "sqlite://C:/Users/Test User/AppData/Roaming/Ludex/ludex.db?mode=rwc",
        )
        .expect("path");
        assert_eq!(
            path.to_string_lossy(),
            "C:/Users/Test User/AppData/Roaming/Ludex/ludex.db"
        );

        let path = sqlite_path_from_url(
            "sqlite://C:\\Users\\Test User\\AppData\\Roaming\\Ludex\\ludex.db?mode=rwc",
        )
        .expect("path");
        assert_eq!(
            path.to_string_lossy(),
            "C:\\Users\\Test User\\AppData\\Roaming\\Ludex\\ludex.db"
        );
    }

    #[test]
    fn percent_encoded_paths_are_decoded() {
        let path = sqlite_path_from_url("sqlite:///tmp/ludex%20data/ludex.db").expect("path");
        assert_eq!(path, PathBuf::from("/tmp/ludex data/ludex.db"));
    }

    #[tokio::test]
    async fn connects_to_a_data_directory_with_spaces() {
        let temp = tempfile::tempdir().expect("tempdir");
        let data_dir = temp.path().join("ludex data");
        std::fs::create_dir_all(&data_dir).expect("mkdir");
        let url = format!("sqlite://{}?mode=rwc", data_dir.join("ludex.db").display());
        let pool = connect(&url).await.expect("connect");
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .expect("query");
        assert_eq!(count, 0);
        pool.close().await;
    }
}
