//! Per-user cloud save storage.
//!
//! Archives are opaque: the server never inspects their contents. Uploads are
//! streamed to a temporary file in the destination directory, hashed on the
//! fly and atomically renamed to
//! `{data_dir}/saves/{user_id}/{game_id}/{snapshot_id}-{file_name}`.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::paths::{self, PathError};

/// Extra body allowance above the configured file limit, covering multipart
/// framing (boundaries, part headers and the optional label field).
pub const MULTIPART_OVERHEAD_BYTES: u64 = 1024 * 1024;

/// Longest sanitized file name, in characters.
const MAX_FILE_NAME_CHARS: usize = 128;

/// Fallback file name when the client sends nothing usable.
const FALLBACK_FILE_NAME: &str = "save.zip";

/// Body limit for the save upload route: the file limit plus framing overhead.
#[must_use]
pub fn body_limit(max_bytes: u64) -> usize {
    usize::try_from(max_bytes.saturating_add(MULTIPART_OVERHEAD_BYTES)).unwrap_or(usize::MAX)
}

/// Root of the cloud save tree.
#[must_use]
pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("saves")
}

/// Directory holding one user's saves for one game.
///
/// Both ids are integers, so their decimal rendering is digits (and possibly a
/// leading `-`) and cannot contain a separator: the join cannot traverse.
#[must_use]
pub fn game_dir(data_dir: &Path, user_id: i64, game_id: i64) -> PathBuf {
    let base = root(data_dir);
    // Numeric ids, no separator possible; see
    // `paths_stay_under_the_user_game_directory`.
    // codeql[rust/path-injection]
    let user = base.join(user_id.to_string());
    // codeql[rust/path-injection]
    user.join(game_id.to_string())
}

/// Final on-disk path of a stored snapshot.
///
/// `file_name` is re-sanitised here even though it was sanitised on upload: the
/// row is the only source at download/delete time, and a tampered row must not
/// become a traversal. The result is confined to `{data_dir}/saves/{user}/{game}`
/// by [`paths::child`]; see `paths_stay_under_the_user_game_directory`.
pub fn snapshot_path(
    data_dir: &Path,
    user_id: i64,
    game_id: i64,
    snapshot_id: i64,
    file_name: &str,
) -> Result<PathBuf, PathError> {
    let name = sanitize_file_name(file_name);
    paths::child(
        &game_dir(data_dir, user_id, game_id),
        &format!("{snapshot_id}-{name}"),
    )
}

/// Reduces a client-supplied file name to a single safe path component.
///
/// Directory components (POSIX and Windows separators), control characters and
/// Windows-reserved characters are replaced, and leading/trailing dots and
/// spaces are stripped so the result can never escape the snapshot directory.
#[must_use]
pub fn sanitize_file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut cleaned = String::with_capacity(base.len());
    for character in base.chars() {
        if character.is_control()
            || matches!(
                character,
                '/' | '\\' | ':' | '"' | '<' | '>' | '|' | '?' | '*'
            )
        {
            cleaned.push('_');
        } else {
            cleaned.push(character);
        }
    }
    let cleaned = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);
    let name: String = cleaned.chars().take(MAX_FILE_NAME_CHARS).collect();
    if name.is_empty() {
        FALLBACK_FILE_NAME.to_string()
    } else {
        name
    }
}

/// Failure while receiving a save upload.
#[derive(Debug)]
pub enum SaveError {
    /// The archive is larger than the configured limit.
    TooLarge,
    /// The archive is empty.
    Empty,
    /// Reading the request body failed.
    Stream(String),
    /// Writing the temporary file failed.
    Io(std::io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("save archive exceeds the server limit"),
            Self::Empty => formatter.write_str("save archive is empty"),
            Self::Stream(message) => write!(formatter, "read save archive: {message}"),
            Self::Io(error) => write!(formatter, "write save archive: {error}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// A fully received upload that has not been published yet.
#[derive(Debug)]
pub struct PendingSave {
    temp_path: PathBuf,
    file_name: String,
    size: u64,
    sha256: String,
}

impl PendingSave {
    /// Sanitized client file name.
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Number of bytes received.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Lowercase hex SHA-256 of the received bytes.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Atomically renames the temporary file to its final snapshot path.
    ///
    /// The temporary file is removed if the rename fails.
    pub async fn commit(self, dir: &Path, snapshot_id: i64) -> std::io::Result<PathBuf> {
        let name = sanitize_file_name(&self.file_name);
        let path = paths::child(dir, &format!("{snapshot_id}-{name}"))
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        // `path` is one sanitised component under the user/game directory
        // (`paths::child`), and `temp_path` is a `.upload-{uuid}.part` file
        // created by `receive` in that same directory; see
        // `paths_stay_under_the_user_game_directory`.
        // codeql[rust/path-injection]
        if let Err(error) = tokio::fs::rename(&self.temp_path, &path).await {
            let _ = tokio::fs::remove_file(&self.temp_path).await;
            return Err(error);
        }
        Ok(path)
    }

    /// Deletes the temporary file.
    pub async fn discard(self) {
        let _ = tokio::fs::remove_file(&self.temp_path).await;
    }
}

/// Streams one multipart file into a temporary file inside `dir`.
///
/// The whole archive is never held in memory: chunks are written as they
/// arrive while SHA-256 is updated incrementally.
pub async fn receive<S, E>(
    dir: &Path,
    file_name: String,
    mut stream: S,
    max_bytes: u64,
) -> Result<PendingSave, SaveError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    // `dir` is built from numeric user/game ids in `game_dir`; see
    // `paths_stay_under_the_user_game_directory`.
    // codeql[rust/path-injection]
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(SaveError::Io)?;
    // The temporary name is a v4 UUID rendered here, so it is a single entry by
    // construction; `dir` is built from numeric ids in `game_dir`.
    let temp_path = dir.join(format!(".upload-{}.part", uuid::Uuid::new_v4()));
    // UUID file name joined onto a numeric-id directory; see
    // `paths_stay_under_the_user_game_directory`.
    // codeql[rust/path-injection]
    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(SaveError::Io)?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;

    let received = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| SaveError::Stream(error.to_string()))?;
            size = size.saturating_add(chunk.len() as u64);
            if size > max_bytes {
                return Err(SaveError::TooLarge);
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(SaveError::Io)?;
        }
        file.flush().await.map_err(SaveError::Io)?;
        file.sync_all().await.map_err(SaveError::Io)?;
        if size == 0 {
            return Err(SaveError::Empty);
        }
        Ok(())
    }
    .await;

    if let Err(error) = received {
        drop(file);
        // Same UUID temporary path.
        // codeql[rust/path-injection]
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error);
    }
    drop(file);

    Ok(PendingSave {
        temp_path,
        file_name,
        size,
        sha256: hex::encode(hasher.finalize()),
    })
}

/// Deletes a stored snapshot and prunes empty parent directories.
pub async fn delete_file(
    data_dir: &Path,
    user_id: i64,
    game_id: i64,
    snapshot_id: i64,
    file_name: &str,
) {
    let path = match snapshot_path(data_dir, user_id, game_id, snapshot_id, file_name) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(error = %error, "refused to delete an out-of-root save path");
            return;
        }
    };
    // `snapshot_path` confines the name to one sanitised component; see
    // `paths_stay_under_the_user_game_directory`.
    // codeql[rust/path-injection]
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(error = %error, path = %path.display(), "could not delete save file");
            return;
        }
    }
    // Best effort: `remove_dir` only succeeds on empty directories. Both paths
    // are built from numeric ids.
    // codeql[rust/path-injection]
    let _ = tokio::fs::remove_dir(game_dir(data_dir, user_id, game_id)).await;
    // codeql[rust/path-injection]
    let _ = tokio::fs::remove_dir(root(data_dir).join(user_id.to_string())).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_path_components_and_controls() {
        assert_eq!(sanitize_file_name("../../etc/passwd.zip"), "passwd.zip");
        assert_eq!(
            sanitize_file_name("..\\..\\windows\\system32\\evil.zip"),
            "evil.zip"
        );
        assert_eq!(sanitize_file_name("/absolute/save.zip"), "save.zip");
        assert_eq!(sanitize_file_name("C:\\Users\\me\\save.zip"), "save.zip");
        assert_eq!(sanitize_file_name("bad\u{0}name.zip"), "bad_name.zip");
        assert_eq!(sanitize_file_name("..."), FALLBACK_FILE_NAME);
        assert_eq!(sanitize_file_name("   "), FALLBACK_FILE_NAME);
        assert_eq!(sanitize_file_name(""), FALLBACK_FILE_NAME);
        assert_eq!(sanitize_file_name(".."), FALLBACK_FILE_NAME);
    }

    #[test]
    fn sanitize_keeps_readable_names_and_caps_length() {
        assert_eq!(
            sanitize_file_name("Hades save (2020).zip"),
            "Hades save (2020).zip"
        );
        assert_eq!(sanitize_file_name(".hidden.zip"), "hidden.zip");
        let long = format!("{}.zip", "a".repeat(500));
        assert_eq!(
            sanitize_file_name(&long).chars().count(),
            MAX_FILE_NAME_CHARS
        );
    }

    #[test]
    fn paths_stay_under_the_user_game_directory() {
        let root = Path::new("/srv/ludex/data");
        let dir = game_dir(root, 7, 42);
        assert_eq!(dir, Path::new("/srv/ludex/data/saves/7/42"));
        let path = snapshot_path(root, 7, 42, 3, "save.zip").expect("path");
        assert_eq!(path, Path::new("/srv/ludex/data/saves/7/42/3-save.zip"));
    }

    /// Evidence for the `// codeql[rust/path-injection]` markers in this
    /// module: a tampered file name is re-sanitised, and the result can never
    /// leave the user/game directory.
    #[test]
    fn snapshot_names_cannot_escape_the_game_directory() {
        let root = Path::new("/srv/ludex/data");
        let path = snapshot_path(root, 7, 42, 3, "../../etc/passwd").expect("path");
        assert_eq!(path, Path::new("/srv/ludex/data/saves/7/42/3-passwd"));
        assert!(path.starts_with(game_dir(root, 7, 42)));

        for hostile in ["..", "../..", "/etc/passwd", "a/b", "a\\b"] {
            let path = snapshot_path(root, 7, 42, 3, hostile).expect("sanitised");
            assert!(
                path.starts_with(game_dir(root, 7, 42)),
                "{hostile:?} escaped to {path:?}"
            );
        }
    }

    #[test]
    fn body_limit_adds_multipart_overhead() {
        assert_eq!(
            body_limit(1024),
            usize::try_from(1024 + MULTIPART_OVERHEAD_BYTES).expect("fits")
        );
    }

    #[tokio::test]
    async fn receive_streams_and_hashes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("saves/1/2");
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from_static(b"hello ")),
            Ok(Bytes::from_static(b"world")),
        ];
        let stream = futures_util::stream::iter(chunks);
        let pending = receive(&dir, "save.zip".to_string(), stream, 1024)
            .await
            .expect("receive");
        assert_eq!(pending.size(), 11);
        assert_eq!(
            pending.sha256(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );

        let path = pending.commit(&dir, 9).await.expect("commit");
        assert_eq!(path, dir.join("9-save.zip"));
        assert_eq!(tokio::fs::read(&path).await.expect("read"), b"hello world");
    }

    #[tokio::test]
    async fn receive_rejects_oversized_and_empty_uploads() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("saves/1/2");

        let stream =
            futures_util::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(vec![0_u8; 32]))]);
        let error = receive(&dir, "save.zip".to_string(), stream, 16)
            .await
            .expect_err("too large");
        assert!(matches!(error, SaveError::TooLarge));

        let stream = futures_util::stream::iter(Vec::<Result<Bytes, std::io::Error>>::new());
        let error = receive(&dir, "save.zip".to_string(), stream, 16)
            .await
            .expect_err("empty");
        assert!(matches!(error, SaveError::Empty));

        // Failed uploads leave no temporary file behind.
        let mut entries = tokio::fs::read_dir(&dir).await.expect("dir");
        assert!(entries.next_entry().await.expect("entry").is_none());
    }
}
