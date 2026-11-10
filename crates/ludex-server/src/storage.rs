//! Object storage abstraction for the media cache (#47).
//!
//! Two backends are supported:
//!
//! - `local` (default): the current behaviour, files in `LUDEX_MEDIA_DIR`
//!   written atomically and streamed by [`crate::media`].
//! - `s3`: any S3-compatible endpoint (AWS, MinIO, Ceph, Backblaze B2, R2)
//!   through the [`object_store`] crate. Reads are streamed through the server
//!   unless `LUDEX_S3_PUBLIC_BASE_URL` is configured, in which case the media
//!   route answers with a `302` to the public object URL.
//!
//! The library volume itself (archives streamed with HTTP Range support) stays
//! on local disk: object storage is used for the media cache and for backup
//! uploads, never as the source of a game download.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::buffered::BufWriter;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, PutOptions};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::media::safe_media_name;

/// Environment variable selecting the backend (`local` or `s3`).
pub const ENV_BACKEND: &str = "LUDEX_STORAGE_BACKEND";
/// Default key prefix inside the bucket.
pub const DEFAULT_KEY_PREFIX: &str = "ludex";

/// Which backend serves the media cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageKind {
    /// Files on disk under `LUDEX_MEDIA_DIR`.
    #[default]
    Local,
    /// S3-compatible object storage.
    S3,
}

impl StorageKind {
    /// Stable label used by the API and by metrics.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
        }
    }
}

impl std::fmt::Display for StorageKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for StorageKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "local" | "filesystem" | "fs" => Ok(Self::Local),
            "s3" | "object" | "object-store" => Ok(Self::S3),
            other => anyhow::bail!("{ENV_BACKEND} must be 'local' or 's3' (got '{other}')"),
        }
    }
}

/// S3 connection settings, parsed from `LUDEX_S3_*` variables.
#[derive(Clone)]
pub struct StorageConfig {
    /// Selected backend.
    pub kind: StorageKind,
    /// Bucket name (required for `s3`).
    pub bucket: Option<String>,
    /// AWS region; S3-compatible endpoints usually accept anything.
    pub region: String,
    /// Custom endpoint, e.g. `http://minio:9000`.
    pub endpoint: Option<String>,
    /// Access key id.
    pub access_key_id: Option<String>,
    /// Secret access key.
    pub secret_access_key: Option<String>,
    /// Path-style requests (MinIO, Ceph); default true.
    pub path_style: bool,
    /// Key prefix inside the bucket.
    pub prefix: String,
    /// Optional public base URL used to redirect media reads.
    pub public_base_url: Option<String>,
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StorageConfig")
            .field("kind", &self.kind)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            // Never print credentials.
            .field("access_key_id", &self.access_key_id.as_ref().map(|_| "***"))
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "***"),
            )
            .field("path_style", &self.path_style)
            .field("prefix", &self.prefix)
            .field("public_base_url", &self.public_base_url)
            .finish()
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            kind: StorageKind::Local,
            bucket: None,
            region: "us-east-1".to_string(),
            endpoint: None,
            access_key_id: None,
            secret_access_key: None,
            path_style: true,
            prefix: String::new(),
            public_base_url: None,
        }
    }
}

impl StorageConfig {
    /// Reads `LUDEX_STORAGE_BACKEND` and `LUDEX_S3_*` from the environment.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// Testable constructor: `lookup` returns the raw value of a variable.
    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self> {
        let read = |lookup: &mut dyn FnMut(&str) -> Option<String>, key: &str| {
            lookup(key)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };

        let kind = read(&mut lookup, ENV_BACKEND)
            .unwrap_or_else(|| "local".to_string())
            .parse::<StorageKind>()?;
        let bool_value = |lookup: &mut dyn FnMut(&str) -> Option<String>, key: &str, default| {
            read(lookup, key).map_or(default, |value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
        };
        let path_style = bool_value(&mut lookup, "LUDEX_S3_PATH_STYLE", true);
        let prefix =
            read(&mut lookup, "LUDEX_S3_PREFIX").unwrap_or_else(|| DEFAULT_KEY_PREFIX.to_string());
        let prefix = normalize_prefix(&prefix)?;
        let public_base_url = read(&mut lookup, "LUDEX_S3_PUBLIC_BASE_URL")
            .map(|value| value.trim_end_matches('/').to_string());

        let config = Self {
            kind,
            bucket: read(&mut lookup, "LUDEX_S3_BUCKET"),
            region: read(&mut lookup, "LUDEX_S3_REGION").unwrap_or_else(|| "us-east-1".to_string()),
            endpoint: read(&mut lookup, "LUDEX_S3_ENDPOINT"),
            access_key_id: read(&mut lookup, "LUDEX_S3_ACCESS_KEY"),
            secret_access_key: read(&mut lookup, "LUDEX_S3_SECRET_KEY"),
            path_style,
            prefix,
            public_base_url,
        };
        config.validate()?;
        Ok(config)
    }

    /// Rejects incomplete S3 settings. A local deployment never needs any of
    /// them, so this only runs when the `s3` backend is selected.
    pub fn validate(&self) -> Result<()> {
        if self.kind != StorageKind::S3 {
            return Ok(());
        }
        if self.bucket.as_deref().unwrap_or_default().is_empty() {
            anyhow::bail!("LUDEX_S3_BUCKET is required when LUDEX_STORAGE_BACKEND=s3");
        }
        if self.access_key_id.as_deref().unwrap_or_default().is_empty() {
            anyhow::bail!("LUDEX_S3_ACCESS_KEY is required when LUDEX_STORAGE_BACKEND=s3");
        }
        if self
            .secret_access_key
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        {
            anyhow::bail!("LUDEX_S3_SECRET_KEY is required when LUDEX_STORAGE_BACKEND=s3");
        }
        if let Some(endpoint) = &self.endpoint {
            let parsed = url::Url::parse(endpoint)
                .with_context(|| format!("LUDEX_S3_ENDPOINT is not a URL: {endpoint}"))?;
            anyhow::ensure!(
                matches!(parsed.scheme(), "http" | "https"),
                "LUDEX_S3_ENDPOINT must use http or https"
            );
        }
        Ok(())
    }

    /// Builds the runtime backend. `media_dir` is only used by the local one.
    pub fn build(&self, media_dir: &Path) -> Result<Arc<dyn StorageBackend>> {
        self.validate()?;
        match self.kind {
            StorageKind::Local => Ok(Arc::new(LocalStorage::new(media_dir))),
            StorageKind::S3 => Ok(Arc::new(ObjectStorage::from_config(self)?)),
        }
    }

    /// Whether a public base URL will make media reads redirect.
    #[must_use]
    pub fn public_reads(&self) -> bool {
        self.kind == StorageKind::S3 && self.public_base_url.is_some()
    }
}

fn normalize_prefix(value: &str) -> Result<String> {
    let trimmed = value.trim_matches('/').trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    anyhow::ensure!(
        !trimmed.split('/').any(|part| part == ".." || part == "."),
        "LUDEX_S3_PREFIX must not contain '.' or '..' segments"
    );
    Ok(trimmed.to_string())
}

/// Media file body returned by [`StorageBackend::read_media`].
pub enum MediaBody {
    /// Fully buffered body (small objects).
    Bytes(Bytes),
    /// Streaming body, already mapped to `std::io::Error`.
    Stream(BoxStream<'static, std::result::Result<Bytes, std::io::Error>>),
}

/// One readable media object.
pub struct StoredMedia {
    /// Content.
    pub body: MediaBody,
    /// Size in bytes.
    pub len: u64,
    /// MIME type.
    pub content_type: String,
}

/// Aggregate statistics for the media cache.
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
    /// Number of cached files/objects.
    pub files: u64,
    /// Total size.
    pub bytes: u64,
}

/// Storage backend used for cached artwork and for backup uploads.
#[async_trait]
pub trait StorageBackend: Send + Sync + std::fmt::Debug {
    /// Backend kind.
    fn kind(&self) -> StorageKind;

    /// Local media directory, when the backend is `local`.
    fn local_dir(&self) -> Option<&Path>;

    /// Public URL for a media file, when reads can bypass the server.
    fn media_public_url(&self, name: &str) -> Option<String>;

    /// Whether a media file exists.
    async fn exists_media(&self, name: &str) -> Result<bool>;

    /// Stores a media file.
    async fn put_media(&self, name: &str, bytes: Bytes, content_type: &str) -> Result<()>;

    /// Reads a media file; `None` when it does not exist.
    async fn read_media(&self, name: &str) -> Result<Option<StoredMedia>>;

    /// Deletes a media file; returns whether it existed.
    async fn delete_media(&self, name: &str) -> Result<bool>;

    /// Media cache statistics. `None` when computing them is too expensive
    /// (S3 would require listing the bucket on every metrics scrape).
    async fn media_stats(&self) -> Result<Option<StorageStats>>;

    /// Uploads an arbitrary local file under `key` (relative, `/` separated).
    async fn put_file(&self, key: &str, path: &Path) -> Result<u64>;
}

/// Rejects object keys that could escape the prefix or the media directory.
#[must_use]
pub fn safe_object_key(key: &str) -> bool {
    if key.is_empty()
        || key.starts_with('/')
        || key.ends_with('/')
        || key.contains('\\')
        || key.contains('\0')
        || key.contains("..")
        || key.chars().any(char::is_control)
    {
        return false;
    }
    key.split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Validates a media name and returns the error message used by the API.
pub(crate) fn check_media_name(name: &str) -> Result<()> {
    anyhow::ensure!(safe_media_name(name), "invalid media name");
    Ok(())
}

// ---------------------------------------------------------------------------
// Local filesystem backend (default)
// ---------------------------------------------------------------------------

/// Current behaviour: artwork files under `LUDEX_MEDIA_DIR`.
#[derive(Debug)]
pub struct LocalStorage {
    media_dir: PathBuf,
}

impl LocalStorage {
    /// Builds a backend rooted at `media_dir`.
    #[must_use]
    pub fn new(media_dir: impl Into<PathBuf>) -> Self {
        Self {
            media_dir: media_dir.into(),
        }
    }
}

#[async_trait]
impl StorageBackend for LocalStorage {
    fn kind(&self) -> StorageKind {
        StorageKind::Local
    }

    fn local_dir(&self) -> Option<&Path> {
        Some(&self.media_dir)
    }

    fn media_public_url(&self, _name: &str) -> Option<String> {
        None
    }

    async fn exists_media(&self, name: &str) -> Result<bool> {
        check_media_name(name)?;
        Ok(tokio::fs::try_exists(self.media_dir.join(name))
            .await
            .unwrap_or(false))
    }

    async fn put_media(&self, name: &str, bytes: Bytes, _content_type: &str) -> Result<()> {
        check_media_name(name)?;
        tokio::fs::create_dir_all(&self.media_dir)
            .await
            .context("create media directory")?;
        let destination = self.media_dir.join(name);
        let temporary = self
            .media_dir
            .join(format!(".tmp-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&temporary, &bytes)
            .await
            .context("write artwork")?;
        match tokio::fs::rename(&temporary, &destination).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                if tokio::fs::try_exists(&destination).await.unwrap_or(false) {
                    Ok(())
                } else {
                    Err(error).context("store artwork")
                }
            }
        }
    }

    async fn read_media(&self, name: &str) -> Result<Option<StoredMedia>> {
        check_media_name(name)?;
        let path = self.media_dir.join(name);
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => return Ok(None),
        };
        let file = tokio::fs::File::open(&path).await.context("open artwork")?;
        let content_type = mime_guess::from_path(&path)
            .first_or_octet_stream()
            .to_string();
        Ok(Some(StoredMedia {
            body: MediaBody::Stream(
                ReaderStream::new(file)
                    .map(|chunk| chunk.map_err(std::io::Error::other))
                    .boxed(),
            ),
            len: metadata.len(),
            content_type,
        }))
    }

    async fn delete_media(&self, name: &str) -> Result<bool> {
        check_media_name(name)?;
        match tokio::fs::remove_file(self.media_dir.join(name)).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).context("delete artwork"),
        }
    }

    async fn media_stats(&self) -> Result<Option<StorageStats>> {
        let mut stats = StorageStats::default();
        let mut entries = match tokio::fs::read_dir(&self.media_dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Some(stats)),
            Err(error) => return Err(error).context("read media directory"),
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !safe_media_name(&name) {
                continue;
            }
            if let Ok(metadata) = entry.metadata().await {
                if metadata.is_file() {
                    stats.files += 1;
                    stats.bytes += metadata.len();
                }
            }
        }
        Ok(Some(stats))
    }

    async fn put_file(&self, key: &str, path: &Path) -> Result<u64> {
        anyhow::ensure!(safe_object_key(key), "invalid object key");
        let destination = self.media_dir.join(key);
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let bytes = tokio::fs::copy(path, &destination)
            .await
            .context("copy file into media directory")?;
        Ok(bytes)
    }
}

// ---------------------------------------------------------------------------
// S3-compatible backend
// ---------------------------------------------------------------------------

/// S3-compatible backend built on `object_store`.
pub struct ObjectStorage {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    public_base_url: Option<String>,
    bucket: String,
}

impl std::fmt::Debug for ObjectStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObjectStorage")
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("public_base_url", &self.public_base_url)
            .finish()
    }
}

impl ObjectStorage {
    /// Builds the S3 client from validated settings.
    pub fn from_config(config: &StorageConfig) -> Result<Self> {
        config.validate()?;
        let bucket = config
            .bucket
            .clone()
            .context("LUDEX_S3_BUCKET is required")?;
        let allow_http = config
            .endpoint
            .as_deref()
            .is_some_and(|endpoint| endpoint.starts_with("http://"));

        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(bucket.clone())
            .with_region(config.region.clone())
            .with_access_key_id(
                config
                    .access_key_id
                    .clone()
                    .context("LUDEX_S3_ACCESS_KEY is required")?,
            )
            .with_secret_access_key(
                config
                    .secret_access_key
                    .clone()
                    .context("LUDEX_S3_SECRET_KEY is required")?,
            )
            .with_virtual_hosted_style_request(!config.path_style)
            .with_allow_http(allow_http);
        if let Some(endpoint) = config.endpoint.as_deref() {
            builder = builder.with_endpoint(endpoint);
        }
        let store = builder.build().context("build S3 client")?;
        Ok(Self {
            store: Arc::new(store),
            prefix: config.prefix.clone(),
            public_base_url: config
                .public_base_url
                .as_ref()
                .map(|base| base.trim_end_matches('/').to_string()),
            bucket,
        })
    }

    /// Builds a backend over an arbitrary `object_store` implementation.
    ///
    /// Used by tests (in-memory or local object store) and by callers that
    /// bring their own client.
    #[must_use]
    pub fn with_store(
        store: Arc<dyn ObjectStore>,
        bucket: impl Into<String>,
        prefix: impl Into<String>,
        public_base_url: Option<String>,
    ) -> Self {
        Self {
            store,
            prefix: normalize_prefix(&prefix.into()).unwrap_or_default(),
            public_base_url: public_base_url.map(|base| base.trim_end_matches('/').to_string()),
            bucket: bucket.into(),
        }
    }

    fn key(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", self.prefix, name)
        }
    }

    fn media_key(&self, name: &str) -> String {
        self.key(&format!("media/{name}"))
    }
}

#[async_trait]
impl StorageBackend for ObjectStorage {
    fn kind(&self) -> StorageKind {
        StorageKind::S3
    }

    fn local_dir(&self) -> Option<&Path> {
        None
    }

    fn media_public_url(&self, name: &str) -> Option<String> {
        self.public_base_url
            .as_ref()
            .map(|base| format!("{base}/{}", self.media_key(name)))
    }

    async fn exists_media(&self, name: &str) -> Result<bool> {
        check_media_name(name)?;
        match self
            .store
            .head(&ObjectPath::from(self.media_key(name)))
            .await
        {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(error) => Err(error).context("head artwork object"),
        }
    }

    async fn put_media(&self, name: &str, bytes: Bytes, content_type: &str) -> Result<()> {
        check_media_name(name)?;
        let mut attributes = object_store::Attributes::new();
        attributes.insert(
            object_store::Attribute::ContentType,
            content_type.to_string().into(),
        );
        let options = PutOptions {
            attributes,
            ..PutOptions::default()
        };
        self.store
            .put_opts(
                &ObjectPath::from(self.media_key(name)),
                bytes.into(),
                options,
            )
            .await
            .context("upload artwork object")?;
        Ok(())
    }

    async fn read_media(&self, name: &str) -> Result<Option<StoredMedia>> {
        check_media_name(name)?;
        let result = match self
            .store
            .get(&ObjectPath::from(self.media_key(name)))
            .await
        {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(error).context("get artwork object"),
        };
        let len = result.meta.size;
        let content_type = result
            .attributes
            .get(&object_store::Attribute::ContentType)
            .map(|value| value.as_ref().to_string())
            .or_else(|| {
                mime_guess::from_path(name)
                    .first()
                    .map(|mime| mime.to_string())
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());
        Ok(Some(StoredMedia {
            body: MediaBody::Stream(
                result
                    .into_stream()
                    .map(|chunk| chunk.map_err(std::io::Error::other))
                    .boxed(),
            ),
            len,
            content_type,
        }))
    }

    async fn delete_media(&self, name: &str) -> Result<bool> {
        check_media_name(name)?;
        match self
            .store
            .delete(&ObjectPath::from(self.media_key(name)))
            .await
        {
            Ok(()) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(error) => Err(error).context("delete artwork object"),
        }
    }

    async fn media_stats(&self) -> Result<Option<StorageStats>> {
        // Listing the bucket on every Prometheus scrape would cost a request
        // per page; cache statistics are only exposed for the local backend.
        Ok(None)
    }

    async fn put_file(&self, key: &str, path: &Path) -> Result<u64> {
        anyhow::ensure!(safe_object_key(key), "invalid object key");
        let metadata = tokio::fs::metadata(path).await?;
        let file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        let mut writer = BufWriter::new(self.store.clone(), ObjectPath::from(self.key(key)));
        let mut reader = tokio::io::BufReader::new(file);
        match tokio::io::copy(&mut reader, &mut writer).await {
            Ok(_) => {
                writer.shutdown().await.context("complete object upload")?;
                Ok(metadata.len())
            }
            Err(error) => {
                let _ = writer.abort().await;
                Err(error).context("stream object upload")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;

    fn memory_storage(public_base: Option<&str>) -> ObjectStorage {
        ObjectStorage::with_store(
            Arc::new(InMemory::new()),
            "ludex-test",
            "ludex",
            public_base.map(str::to_string),
        )
    }

    #[tokio::test]
    async fn object_backend_put_get_delete_round_trip() {
        let storage = memory_storage(None);
        assert_eq!(storage.kind(), StorageKind::S3);
        assert!(!storage.exists_media("cover.jpg").await.unwrap());

        storage
            .put_media(
                "cover.jpg",
                Bytes::from_static(b"cover-bytes"),
                "image/jpeg",
            )
            .await
            .unwrap();
        assert!(storage.exists_media("cover.jpg").await.unwrap());

        let stored = storage.read_media("cover.jpg").await.unwrap().unwrap();
        assert_eq!(stored.len, 11);
        assert_eq!(stored.content_type, "image/jpeg");
        let bytes = match stored.body {
            MediaBody::Bytes(bytes) => bytes,
            MediaBody::Stream(stream) => {
                let collected: Vec<Bytes> = stream.map(|item| item.unwrap()).collect().await;
                Bytes::from(collected.concat())
            }
        };
        assert_eq!(&bytes[..], b"cover-bytes");

        assert!(storage.delete_media("cover.jpg").await.unwrap());
        // The in-memory store treats a second delete as a success; what
        // matters is that the object is gone.
        let _ = storage.delete_media("cover.jpg").await;
        assert!(storage.read_media("cover.jpg").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn object_backend_exposes_public_urls_with_prefix() {
        let storage = memory_storage(Some("https://cdn.example.com/ludex"));
        assert_eq!(
            storage.media_public_url("abc.jpg").as_deref(),
            Some("https://cdn.example.com/ludex/ludex/media/abc.jpg")
        );
        assert!(memory_storage(None).media_public_url("abc.jpg").is_none());
    }

    #[tokio::test]
    async fn local_backend_is_the_default_behaviour() {
        let temp = tempfile::tempdir().unwrap();
        let storage = LocalStorage::new(temp.path());
        assert_eq!(storage.kind(), StorageKind::Local);
        assert!(storage.media_public_url("cover.jpg").is_none());
        assert!(!storage.exists_media("cover.jpg").await.unwrap());

        storage
            .put_media("cover.jpg", Bytes::from_static(b"abc"), "image/jpeg")
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read(temp.path().join("cover.jpg"))
                .await
                .unwrap(),
            b"abc"
        );
        let stats = storage.media_stats().await.unwrap().unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(stats.bytes, 3);
        assert!(storage.delete_media("cover.jpg").await.unwrap());
        assert!(!storage.delete_media("cover.jpg").await.unwrap());
    }

    #[tokio::test]
    async fn media_names_cannot_escape_the_backend() {
        let temp = tempfile::tempdir().unwrap();
        let storage = LocalStorage::new(temp.path());
        assert!(storage
            .put_media("../evil.jpg", Bytes::new(), "image/jpeg")
            .await
            .is_err());
        assert!(storage.read_media("dir/evil.jpg").await.is_err());
        assert!(storage.exists_media("").await.is_err());
    }

    #[tokio::test]
    async fn put_file_uploads_by_key() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("backup.tar.gz");
        tokio::fs::write(&source, b"archive").await.unwrap();
        let storage = memory_storage(None);
        let written = storage
            .put_file("backups/one.tar.gz", &source)
            .await
            .unwrap();
        assert_eq!(written, 7);
        assert!(storage.put_file("../escape", &source).await.is_err());
    }

    #[test]
    fn storage_config_defaults_to_local() {
        let config = StorageConfig::from_lookup(|_| None).unwrap();
        assert_eq!(config.kind, StorageKind::Local);
        assert_eq!(config.region, "us-east-1");
        assert!(config.path_style);
        assert_eq!(config.prefix, DEFAULT_KEY_PREFIX);
        assert!(!config.public_reads());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn storage_config_parses_s3_settings() {
        let values = std::collections::HashMap::from([
            ("LUDEX_STORAGE_BACKEND", "s3"),
            ("LUDEX_S3_BUCKET", "ludex"),
            ("LUDEX_S3_REGION", "eu-west-3"),
            ("LUDEX_S3_ENDPOINT", "http://minio:9000"),
            ("LUDEX_S3_ACCESS_KEY", "key"),
            ("LUDEX_S3_SECRET_KEY", "secret"),
            ("LUDEX_S3_PATH_STYLE", "false"),
            ("LUDEX_S3_PREFIX", "/art/"),
            ("LUDEX_S3_PUBLIC_BASE_URL", "https://cdn.example.com/"),
        ]);
        let config =
            StorageConfig::from_lookup(|key| values.get(key).map(|value| value.to_string()))
                .unwrap();
        assert_eq!(config.kind, StorageKind::S3);
        assert_eq!(config.bucket.as_deref(), Some("ludex"));
        assert_eq!(config.region, "eu-west-3");
        assert_eq!(config.endpoint.as_deref(), Some("http://minio:9000"));
        assert!(!config.path_style);
        assert_eq!(config.prefix, "art");
        assert_eq!(
            config.public_base_url.as_deref(),
            Some("https://cdn.example.com")
        );
        assert!(config.public_reads());
    }

    #[test]
    fn storage_config_rejects_incomplete_s3() {
        let values = std::collections::HashMap::from([("LUDEX_STORAGE_BACKEND", "s3")]);
        let error =
            StorageConfig::from_lookup(|key| values.get(key).map(|value| value.to_string()))
                .unwrap_err();
        assert!(error.to_string().contains("LUDEX_S3_BUCKET"));

        let values = std::collections::HashMap::from([
            ("LUDEX_STORAGE_BACKEND", "s3"),
            ("LUDEX_S3_BUCKET", "ludex"),
            ("LUDEX_S3_ACCESS_KEY", "key"),
            ("LUDEX_S3_SECRET_KEY", "secret"),
            ("LUDEX_S3_ENDPOINT", "minio:9000"),
        ]);
        let error =
            StorageConfig::from_lookup(|key| values.get(key).map(|value| value.to_string()))
                .unwrap_err();
        assert!(error.to_string().contains("LUDEX_S3_ENDPOINT"));
    }

    #[test]
    fn storage_config_rejects_unknown_backend() {
        let values = std::collections::HashMap::from([("LUDEX_STORAGE_BACKEND", "ftp")]);
        let error =
            StorageConfig::from_lookup(|key| values.get(key).map(|value| value.to_string()))
                .unwrap_err();
        assert!(error.to_string().contains("must be 'local' or 's3'"));
    }

    #[test]
    fn storage_config_rejects_traversal_prefix() {
        let values = std::collections::HashMap::from([
            ("LUDEX_STORAGE_BACKEND", "s3"),
            ("LUDEX_S3_BUCKET", "ludex"),
            ("LUDEX_S3_ACCESS_KEY", "key"),
            ("LUDEX_S3_SECRET_KEY", "secret"),
            ("LUDEX_S3_PREFIX", "../escape"),
        ]);
        assert!(
            StorageConfig::from_lookup(|key| values.get(key).map(|value| value.to_string()))
                .is_err()
        );
    }

    #[test]
    fn object_keys_are_validated() {
        assert!(safe_object_key("media/abc.jpg"));
        assert!(safe_object_key("backups/2026/one.tar.gz"));
        assert!(!safe_object_key("../etc/passwd"));
        assert!(!safe_object_key("/absolute"));
        assert!(!safe_object_key("a//b"));
        assert!(!safe_object_key("a\\b"));
        assert!(!safe_object_key(""));
    }

    #[test]
    fn debug_never_prints_credentials() {
        let config = StorageConfig {
            kind: StorageKind::S3,
            bucket: Some("ludex".into()),
            access_key_id: Some("AKIAEXAMPLE".into()),
            secret_access_key: Some("super-secret".into()),
            ..StorageConfig::default()
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("super-secret"));
        assert!(!rendered.contains("AKIAEXAMPLE"));
    }
}
