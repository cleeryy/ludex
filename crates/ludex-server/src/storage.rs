//! Game archive storage: local library roots or an S3-compatible bucket (#47).
//!
//! The abstraction is deliberately small: archives are uploaded once, then read
//! with HTTP Range semantics. The local library file is never deleted by a scan,
//! so it remains the fallback for rows that were indexed before S3 was enabled
//! or whose upload failed; only the explicit editor deletion route (#168)
//! removes a version's local file and object.
//!
//! Metadata, artwork and cloud saves stay on local disk: only the game archives
//! travel to object storage. See `docs/STORAGE.md`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::buffered::BufWriter;
use object_store::path::Path as ObjectPath;
use object_store::{BackoffConfig, ObjectStoreExt};
use object_store::{GetOptions, GetRange, ObjectStore, RetryConfig};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::config::{Config, S3Config, StorageBackend};

/// Read range requested by the client, in HTTP terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestRange {
    /// `bytes=start-end` (inclusive end).
    Bounded {
        /// First byte.
        start: u64,
        /// Last byte, inclusive.
        end: u64,
    },
    /// `bytes=start-`.
    Offset(u64),
    /// `bytes=-length`.
    Suffix(u64),
}

/// Object returned by [`ArchiveStorage::publish_archive`].
#[derive(Debug, Clone)]
pub struct StoredObject {
    /// Backend that accepted the upload.
    pub backend: StorageBackend,
    /// Key inside the bucket, already prefixed.
    pub key: String,
    /// ETag reported by the object store, when available.
    pub etag: Option<String>,
    /// Object size in bytes.
    pub size: u64,
}

/// Streaming read of an object.
pub struct ObjectRead {
    /// Full object size.
    pub size: u64,
    /// Inclusive byte range in the response, when a Range was requested.
    pub range: Option<(u64, u64)>,
    /// Body stream.
    pub body: axum::body::Body,
}

/// Storage failures, mapped to HTTP status by the routes.
#[derive(Debug)]
pub enum StorageError {
    /// The object does not exist in the bucket.
    NotFound,
    /// The requested range cannot be satisfied.
    RangeNotSatisfiable,
    /// The backend is disabled (local mode).
    Disabled,
    /// Any other upstream failure. The string is logged, never returned.
    Unavailable(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("object not found"),
            Self::RangeNotSatisfiable => formatter.write_str("requested range is not satisfiable"),
            Self::Disabled => formatter.write_str("object storage is not enabled"),
            Self::Unavailable(message) => {
                write!(formatter, "object storage unavailable: {message}")
            }
        }
    }
}

impl std::error::Error for StorageError {}

fn unavailable(error: object_store::Error) -> StorageError {
    match error {
        object_store::Error::NotFound { .. } => StorageError::NotFound,
        other => StorageError::Unavailable(other.to_string()),
    }
}

struct Inner {
    backend: StorageBackend,
    store: Option<Arc<dyn ObjectStore>>,
    /// Key prefix, already normalised (no surrounding slashes).
    prefix: Option<String>,
}

/// Archive storage shared by the scanner and the download routes.
#[derive(Clone)]
pub struct ArchiveStorage {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ArchiveStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArchiveStorage")
            .field("backend", &self.inner.backend)
            .field("remote", &self.is_remote())
            .finish()
    }
}

impl ArchiveStorage {
    /// Builds the storage from configuration. Never performs a network call.
    ///
    /// A malformed S3 configuration logs a warning and leaves the server on
    /// local storage instead of preventing startup.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        if config.storage_backend != StorageBackend::S3 {
            return Self::local();
        }
        let Some(s3) = config.s3.clone() else {
            tracing::warn!("LUDEX_STORAGE_BACKEND=s3 without S3 settings; using local storage");
            return Self::local();
        };
        match build_s3(&s3) {
            Ok(store) => {
                tracing::info!(
                    endpoint = %display_endpoint(s3.endpoint.as_deref()),
                    bucket = %s3.bucket,
                    path_style = s3.path_style,
                    "game archives will be uploaded to S3"
                );
                Self {
                    inner: Arc::new(Inner {
                        backend: StorageBackend::S3,
                        store: Some(store),
                        prefix: s3.prefix.clone(),
                    }),
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not configure S3 storage; falling back to local");
                Self::local()
            }
        }
    }

    /// Local-only storage.
    #[must_use]
    pub fn local() -> Self {
        Self {
            inner: Arc::new(Inner {
                backend: StorageBackend::Local,
                store: None,
                prefix: None,
            }),
        }
    }

    /// Wraps an arbitrary object store. Used by tests and custom integrations.
    #[must_use]
    pub fn from_store(store: Arc<dyn ObjectStore>) -> Self {
        Self::from_store_with_prefix(store, None)
    }

    /// Wraps an object store with an explicit key prefix (tests, integrations).
    #[must_use]
    pub fn from_store_with_prefix(store: Arc<dyn ObjectStore>, prefix: Option<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend: StorageBackend::S3,
                store: Some(store),
                prefix: prefix.map(|prefix| prefix.trim_matches('/').to_string()),
            }),
        }
    }

    /// Whether uploads and proxied reads are enabled.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.inner.backend == StorageBackend::S3 && self.inner.store.is_some()
    }

    /// Backend identifier stored on `game_versions.storage_backend`.
    #[must_use]
    pub fn backend(&self) -> StorageBackend {
        self.inner.backend
    }

    /// Uploads a local archive and returns its key and ETag.
    ///
    /// `identity` must change when the file is replaced (the scanner uses
    /// root id, relative path, size and mtime): the object key is derived from
    /// it so an archive is uploaded once per content generation.
    pub async fn publish_archive(
        &self,
        identity: &str,
        file_name: &str,
        path: &Path,
    ) -> Result<Option<StoredObject>, StorageError> {
        let Some(store) = self.inner.store.as_ref() else {
            return Ok(None);
        };
        let digest = Sha256::digest(identity.as_bytes());
        let stem = hex::encode(&digest[..12]);
        let key = self.full_key(&format!(
            "archives/{stem}-{}",
            sanitize_key_component(file_name)
        ));
        let location = ObjectPath::from(key.as_str());

        let file = tokio::fs::File::open(path)
            .await
            .map_err(|error| StorageError::Unavailable(format!("open archive: {error}")))?;
        let mut reader = tokio::io::BufReader::new(file);
        let mut writer = BufWriter::new(Arc::clone(store), location.clone());
        tokio::io::copy(&mut reader, &mut writer)
            .await
            .map_err(|error| StorageError::Unavailable(format!("upload archive: {error}")))?;
        writer
            .shutdown()
            .await
            .map_err(|error| StorageError::Unavailable(format!("finish upload: {error}")))?;

        let meta = store.head(&location).await.map_err(unavailable)?;
        Ok(Some(StoredObject {
            backend: StorageBackend::S3,
            key,
            etag: meta.e_tag.clone(),
            size: meta.size,
        }))
    }

    /// Deletes an archive object from the bucket (#168).
    ///
    /// Only called by the version deletion route, with an explicit editor
    /// confirmation: a missing object is treated as already deleted so the
    /// database row can still be removed.
    pub async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let store = self.inner.store.as_ref().ok_or(StorageError::Disabled)?;
        let location = ObjectPath::from(key);
        match store.delete(&location).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(error) => Err(unavailable(error)),
        }
    }

    /// Reads an object's metadata without transferring its body (#165).
    ///
    /// Returns the object size; [`StorageError::NotFound`] means the key is
    /// absent from the bucket.
    pub async fn head(&self, key: &str) -> Result<u64, StorageError> {
        let store = self.inner.store.as_ref().ok_or(StorageError::Disabled)?;
        let location = ObjectPath::from(key);
        let meta = store.head(&location).await.map_err(unavailable)?;
        Ok(meta.size)
    }

    /// Opens an object, optionally bounded by an HTTP Range.
    pub async fn open(
        &self,
        key: &str,
        range: Option<RequestRange>,
    ) -> Result<ObjectRead, StorageError> {
        let store = self.inner.store.as_ref().ok_or(StorageError::Disabled)?;
        let location = ObjectPath::from(key);
        let meta = store.head(&location).await.map_err(unavailable)?;
        let size = meta.size;

        let (get_range, response_range) = match range {
            None => (None, None),
            Some(RequestRange::Bounded { start, end }) if start <= end && start < size => {
                let end = end.min(size.saturating_sub(1));
                (
                    Some(GetRange::Bounded(start..end.saturating_add(1))),
                    Some((start, end)),
                )
            }
            Some(RequestRange::Offset(start)) if start < size => (
                Some(GetRange::Offset(start)),
                Some((start, size.saturating_sub(1))),
            ),
            Some(RequestRange::Suffix(length)) if length > 0 && size > 0 => {
                let length = length.min(size);
                (
                    Some(GetRange::Suffix(length)),
                    Some((size - length, size - 1)),
                )
            }
            Some(_) => return Err(StorageError::RangeNotSatisfiable),
        };

        let options = GetOptions {
            range: get_range,
            ..GetOptions::default()
        };
        let result = store
            .get_opts(&location, options)
            .await
            .map_err(unavailable)?;
        let body = axum::body::Body::from_stream(
            result
                .into_stream()
                .map(|item| item.map_err(std::io::Error::other)),
        );
        Ok(ObjectRead {
            size,
            range: response_range,
            body,
        })
    }

    /// Downloads an object to a local file. Used to stage S3 archives for
    /// stream-install (`tar`) requests, which need random access.
    pub async fn download_to_path(&self, key: &str, path: &Path) -> Result<u64, StorageError> {
        let store = self.inner.store.as_ref().ok_or(StorageError::Disabled)?;
        let location = ObjectPath::from(key);
        let result = store.get(&location).await.map_err(unavailable)?;
        let expected = result.meta.size;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                StorageError::Unavailable(format!("create staging dir: {error}"))
            })?;
        }
        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(|error| StorageError::Unavailable(format!("create staging file: {error}")))?;
        let mut stream = result.into_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(unavailable)?;
            file.write_all(&chunk).await.map_err(|error| {
                StorageError::Unavailable(format!("write staging file: {error}"))
            })?;
        }
        file.flush()
            .await
            .map_err(|error| StorageError::Unavailable(format!("flush staging file: {error}")))?;
        Ok(expected)
    }

    /// Uploads a backup tarball. `key` is prefixed with `LUDEX_S3_PREFIX`.
    pub async fn upload_file(
        &self,
        key: &str,
        path: &Path,
    ) -> Result<Option<StoredObject>, StorageError> {
        let Some(store) = self.inner.store.as_ref() else {
            return Ok(None);
        };
        let key = self.full_key(key);
        let location = ObjectPath::from(key.as_str());
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|error| StorageError::Unavailable(format!("open upload: {error}")))?;
        let mut reader = tokio::io::BufReader::new(file);
        let mut writer = BufWriter::new(Arc::clone(store), location.clone());
        tokio::io::copy(&mut reader, &mut writer)
            .await
            .map_err(|error| StorageError::Unavailable(format!("upload file: {error}")))?;
        writer
            .shutdown()
            .await
            .map_err(|error| StorageError::Unavailable(format!("finish upload: {error}")))?;
        let meta = store.head(&location).await.map_err(unavailable)?;
        Ok(Some(StoredObject {
            backend: StorageBackend::S3,
            key,
            etag: meta.e_tag.clone(),
            size: meta.size,
        }))
    }

    /// Prefixes a relative key once. Object keys stored in the database are
    /// always full keys, including the prefix: `open` never prefixes again.
    fn full_key(&self, relative: &str) -> String {
        let relative = relative.trim_start_matches('/');
        match self
            .inner
            .prefix
            .as_deref()
            .filter(|prefix| !prefix.is_empty())
        {
            Some(prefix) => format!("{prefix}/{relative}"),
            None => relative.to_string(),
        }
    }
}

/// Removes any userinfo from an endpoint before it reaches the logs.
fn display_endpoint(endpoint: Option<&str>) -> String {
    match endpoint {
        None => "aws".to_string(),
        Some(endpoint) => match url::Url::parse(endpoint) {
            Ok(mut url) => {
                if !url.username().is_empty() {
                    let _ = url.set_username("<redacted>");
                }
                if url.password().is_some() {
                    let _ = url.set_password(Some("<redacted>"));
                }
                url.to_string()
            }
            Err(_) => "<custom>".to_string(),
        },
    }
}

fn build_s3(s3: &S3Config) -> Result<Arc<dyn ObjectStore>, object_store::Error> {
    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(s3.bucket.clone())
        .with_region(s3.region.clone())
        .with_access_key_id(s3.access_key.clone())
        .with_secret_access_key(s3.secret_key.clone())
        .with_virtual_hosted_style_request(!s3.path_style)
        .with_retry(RetryConfig {
            backoff: BackoffConfig::default(),
            max_retries: 2,
            retry_timeout: Duration::from_secs(20),
        });
    if let Some(endpoint) = s3.endpoint.as_deref() {
        builder = builder.with_endpoint(endpoint);
        if endpoint.starts_with("http://") {
            builder = builder.with_allow_http(true);
        }
    }
    Ok(Arc::new(builder.build()?))
}

/// Keeps object keys readable and free of path separators.
fn sanitize_key_component(value: &str) -> String {
    let mut cleaned = String::with_capacity(value.len().min(120));
    for character in value.chars().take(120) {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
            cleaned.push(character);
        } else {
            cleaned.push('_');
        }
    }
    let cleaned = cleaned.trim_matches('.');
    if cleaned.is_empty() {
        "archive".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Collects a streamed body into bytes. Test helper, capped to avoid runaway.
#[cfg(test)]
async fn collect(body: axum::body::Body) -> Vec<u8> {
    use http_body_util::BodyExt;
    body.collect().await.expect("body").to_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use std::io::Write;

    fn temp_archive(payload: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("Cat Quest II.zip");
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(payload).expect("write");
        (dir, path)
    }

    #[tokio::test]
    async fn uploads_and_reads_ranges_from_an_object_store() {
        let storage = ArchiveStorage::from_store(Arc::new(InMemory::new()));
        let payload = b"0123456789abcdef";
        let (_dir, path) = temp_archive(payload);

        let stored = storage
            .publish_archive("root:1:size:42", "Cat Quest II.zip", &path)
            .await
            .expect("upload")
            .expect("remote object");
        assert_eq!(stored.backend, StorageBackend::S3);
        assert_eq!(stored.size, payload.len() as u64);
        assert!(stored.key.starts_with("archives/"));
        assert!(stored.etag.is_some(), "etag should be reported");

        let object = storage
            .open(
                &stored.key,
                Some(RequestRange::Bounded { start: 2, end: 5 }),
            )
            .await
            .expect("ranged read");
        assert_eq!(object.size, payload.len() as u64);
        assert_eq!(object.range, Some((2, 5)));
        assert_eq!(collect(object.body).await, b"2345");

        let object = storage
            .open(&stored.key, Some(RequestRange::Suffix(4)))
            .await
            .expect("suffix read");
        assert_eq!(object.range, Some((12, 15)));
        assert_eq!(collect(object.body).await, b"cdef");

        let object = storage
            .open(&stored.key, Some(RequestRange::Offset(10)))
            .await
            .expect("offset read");
        assert_eq!(object.range, Some((10, 15)));
        assert_eq!(collect(object.body).await, b"abcdef");
    }

    #[tokio::test]
    async fn missing_objects_are_reported_and_bad_ranges_rejected() {
        let storage = ArchiveStorage::from_store(Arc::new(InMemory::new()));
        let error = storage.open("archives/missing.zip", None).await;
        assert!(matches!(error, Err(StorageError::NotFound)));

        let (_dir, path) = temp_archive(b"abc");
        let stored = storage
            .publish_archive("identity", "a.zip", &path)
            .await
            .expect("upload")
            .expect("object");
        let error = storage
            .open(
                &stored.key,
                Some(RequestRange::Bounded { start: 10, end: 20 }),
            )
            .await;
        assert!(matches!(error, Err(StorageError::RangeNotSatisfiable)));
    }

    #[tokio::test]
    async fn prefix_is_applied_once() {
        let storage = ArchiveStorage::from_store_with_prefix(
            Arc::new(InMemory::new()),
            Some("prod/games".to_string()),
        );
        let (_dir, path) = temp_archive(b"prefixed");
        let stored = storage
            .publish_archive("identity", "a.zip", &path)
            .await
            .expect("upload")
            .expect("object");
        assert!(
            stored.key.starts_with("prod/games/archives/"),
            "{}",
            stored.key
        );
        let object = storage.open(&stored.key, None).await.expect("read");
        assert_eq!(collect(object.body).await, b"prefixed");
    }

    #[tokio::test]
    async fn local_storage_never_uploads() {
        let storage = ArchiveStorage::local();
        assert!(!storage.is_remote());
        let (_dir, path) = temp_archive(b"data");
        assert!(storage
            .publish_archive("identity", "a.zip", &path)
            .await
            .expect("local no-op")
            .is_none());
        assert!(matches!(
            storage.open("anything", None).await,
            Err(StorageError::Disabled)
        ));
    }

    #[tokio::test]
    async fn download_to_path_round_trips() {
        let storage = ArchiveStorage::from_store(Arc::new(InMemory::new()));
        let payload = vec![7_u8; 4096];
        let (_dir, path) = temp_archive(&payload);
        let stored = storage
            .publish_archive("identity", "a.zip", &path)
            .await
            .expect("upload")
            .expect("object");
        let out = tempfile::tempdir().expect("tempdir");
        let target = out.path().join("staged.zip");
        let size = storage
            .download_to_path(&stored.key, &target)
            .await
            .expect("download");
        assert_eq!(size, payload.len() as u64);
        assert_eq!(std::fs::read(&target).expect("read"), payload);
    }

    #[test]
    fn keys_are_sanitized() {
        assert_eq!(
            sanitize_key_component("Cat Quest II.zip"),
            "Cat_Quest_II.zip"
        );
        assert_eq!(sanitize_key_component("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_key_component(""), "archive");
    }
}
