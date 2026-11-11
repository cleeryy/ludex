//! Resumable archive uploads: on-disk layout, assembly and cleanup.
//!
//! Chunks are written as `{data}/uploads/{id}/{part}.part`. Each upload row in
//! `uploads` is an index over that directory; deleting the directory makes the
//! row useless, so both are removed together. An upload only becomes a library
//! game after `complete` verified its size and optional SHA-256 and handed the
//! assembled archive to the scanner.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::db;
use crate::models::{part_len, total_parts};
use crate::state::AppState;

/// Directory holding all in-flight uploads.
#[must_use]
pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("uploads")
}

/// Directory holding one upload's chunks.
#[must_use]
pub fn upload_dir(data_dir: &Path, id: &str) -> PathBuf {
    root(data_dir).join(id)
}

/// Path of one chunk.
#[must_use]
pub fn part_path(data_dir: &Path, id: &str, part: i64) -> PathBuf {
    upload_dir(data_dir, id).join(format!("{part}.part"))
}

/// Chunk indices currently stored, ascending.
#[must_use]
pub fn received_parts(dir: &Path, expected: i64) -> Vec<i64> {
    let mut parts = Vec::new();
    let Ok(mut entries) = std::fs::read_dir(dir) else {
        return parts;
    };
    while let Some(Ok(entry)) = entries.next() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(index) = name
            .strip_suffix(".part")
            .and_then(|it| it.parse::<i64>().ok())
        else {
            continue;
        };
        if (0..expected).contains(&index) && entry.metadata().is_ok_and(|meta| meta.is_file()) {
            parts.push(index);
        }
    }
    parts.sort_unstable();
    parts
}

/// Bytes received for an upload, derived from the stored chunks.
#[must_use]
pub fn received_bytes(total: i64, chunk: i64, parts: &[i64]) -> i64 {
    parts
        .iter()
        .map(|index| part_len(total, chunk, *index))
        .sum()
}

/// Sanitises a client file name to a single path component.
#[must_use]
pub fn sanitize_file_name(raw: &str) -> String {
    let name = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("upload")
        .trim()
        .trim_matches('.');
    let mut cleaned: String = name
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    if cleaned.is_empty() {
        cleaned = "upload".to_string();
    }
    if cleaned.chars().count() > 200 {
        let extension = Path::new(&cleaned)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_default();
        let stem: String = cleaned
            .chars()
            .take(200 - extension.chars().count())
            .collect();
        cleaned = format!("{stem}{extension}");
    }
    cleaned
}

/// Picks a non-colliding destination inside the library root.
#[must_use]
pub fn unique_destination(games_dir: &Path, file_name: &str) -> PathBuf {
    let candidate = games_dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("upload");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    for suffix in 2..1000 {
        let name = format!("{stem} ({suffix}){extension}");
        let candidate = games_dir.join(&name);
        if !candidate.exists() {
            return candidate;
        }
    }
    games_dir.join(format!("{stem}-{}{extension}", uuid::Uuid::new_v4()))
}

/// Assembles `parts` into `destination`, returning `(size, sha256)`.
///
/// The caller is responsible for verifying the result before publishing it.
pub async fn assemble(
    upload_dir: &Path,
    total: i64,
    chunk: i64,
    destination: &Path,
) -> Result<(i64, String)> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut output = tokio::fs::File::create(destination).await?;
    let mut hasher = Sha256::new();
    let mut written: i64 = 0;
    for index in 0..total_parts(total, chunk) {
        let path = upload_dir.join(format!("{index}.part"));
        let mut input = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("open chunk {index}"))?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let read = input.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            output.write_all(&buffer[..read]).await?;
            written += i64::try_from(read).unwrap_or(i64::MAX);
        }
    }
    output.flush().await?;
    output.sync_all().await?;
    Ok((written, hex::encode(hasher.finalize())))
}

/// Deletes an upload's chunks and its row.
pub async fn remove(state: &AppState, id: &str) -> Result<()> {
    let dir = upload_dir(&state.config.data_dir, id);
    if let Err(error) = tokio::fs::remove_dir_all(&dir).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.into());
        }
    }
    db::query("DELETE FROM uploads WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Removes unfinished uploads older than `max_age`, plus orphan directories.
///
/// Returns the number of uploads removed.
pub async fn cleanup_stale(state: &AppState, max_age: Duration) -> Result<usize> {
    let cutoff =
        db::now() - chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::zero());
    let cutoff_text = db::db_time(cutoff);
    let stale: Vec<String> = db::query_scalar(
        "SELECT id FROM uploads
         WHERE status != 'complete' AND updated_at < ?",
    )
    .bind(&cutoff_text)
    .fetch_all(&state.pool)
    .await?;

    let mut removed = 0usize;
    for id in &stale {
        remove(state, id).await?;
        removed += 1;
    }

    // Directories without a row (crash between mkdir and insert) are removed
    // once their mtime is old enough.
    let root = root(&state.config.data_dir);
    let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
        return Ok(removed);
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let tracked = db::query_scalar::<i64>("SELECT COUNT(*) FROM uploads WHERE id = ?")
            .bind(&name)
            .fetch_one(&state.pool)
            .await
            .unwrap_or(0);
        if tracked > 0 {
            continue;
        }
        let old = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
    Ok(removed)
}

/// Hourly cleanup loop. Runs until the process exits.
pub async fn cleanup_worker(state: AppState) {
    let interval = state.config.upload_ttl.max(Duration::from_secs(60));
    loop {
        tokio::time::sleep(interval).await;
        if let Err(error) = cleanup_stale(&state, state.config.upload_ttl).await {
            tracing::warn!(error = %error, "upload cleanup failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_math_handles_a_partial_last_chunk() {
        assert_eq!(total_parts(10, 4), 3);
        assert_eq!(part_len(10, 4, 0), 4);
        assert_eq!(part_len(10, 4, 2), 2);
        assert_eq!(part_len(10, 4, 3), 0);
        assert_eq!(total_parts(0, 4), 0);
    }

    #[test]
    fn file_names_cannot_escape_the_library_root() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("C:\\Games\\Foo.zip"), "Foo.zip");
        assert_eq!(sanitize_file_name("  "), "upload");
        assert_eq!(sanitize_file_name("bad:name?.zip"), "bad_name_.zip");
    }

    #[tokio::test]
    async fn assemble_concatenates_chunks_and_hashes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("up");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("0.part"), b"hello ").expect("part");
        std::fs::write(dir.join("1.part"), b"world").expect("part");
        let destination = temp.path().join("out.zip");
        let (size, hash) = assemble(&dir, 11, 6, &destination).await.expect("assemble");
        assert_eq!(size, 11);
        assert_eq!(std::fs::read(&destination).expect("read"), b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
