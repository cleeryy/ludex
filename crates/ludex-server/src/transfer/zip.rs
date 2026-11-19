//! ZIP manifest and per-entry transport (#115, #231).

use std::path::PathBuf;

use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ludex_core::archive::{
    detect_format, zip_entry_info, zip_entry_is_stored, zip_stream_manifest, ArchiveError,
    ArchiveFormat, StreamEntry, StreamManifest,
};

use crate::error::ApiError;
use crate::media::content_disposition;
use crate::models::VersionRow;
use crate::state::AppState;
use crate::storage::RequestRange;
use crate::stream::entry_body;

use super::object::{cleanup_staged, resolve_archive_path};
use super::range::parse_range;
use super::{map_archive_error, stream_unsupported};

/// A resolvable archive path for the stream endpoints: either the library file
/// or an object-store archive staged under `{data_dir}/tmp`.
///
/// Shared with the extracted cache (#231), which extracts the same source.
pub(crate) struct StreamSource {
    pub(crate) path: PathBuf,
    pub(crate) staged: bool,
    pub(crate) format: ArchiveFormat,
}

/// Resolves the archive of a version and detects its format.
///
/// Object-store archives are staged (the tar/manifest readers need random
/// access) and removed by the caller/response once it is done with them.
pub(crate) async fn resolve_stream_source(
    state: &AppState,
    version: &VersionRow,
) -> Result<StreamSource, ApiError> {
    let mut staged = false;
    let path = resolve_archive_path(state, version, &mut staged).await?;

    let format = match detect_format(&path) {
        Ok(format) => format,
        Err(error) => {
            cleanup_staged(&path, staged).await;
            return Err(if error.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_found("file_not_found", "The library file is missing from disk")
            } else {
                ApiError::unprocessable(
                    "stream_failed",
                    format!("Could not inspect the archive: {error}"),
                )
            });
        }
    };
    Ok(StreamSource {
        path,
        staged,
        format,
    })
}

/// Ordered ZIP manifest for a resumable per-entry install.
///
/// Non-ZIP archives answer `415 stream_unsupported`, which is the client's
/// signal to fall back to the sequential tar stream or the normal resumable
/// download.
pub(crate) async fn manifest(
    state: &AppState,
    version: &VersionRow,
) -> Result<Json<StreamManifest>, ApiError> {
    let source = resolve_stream_source(state, version).await?;

    if source.format != ArchiveFormat::Zip {
        cleanup_staged(&source.path, source.staged).await;
        return Err(stream_unsupported());
    }

    let manifest_path = source.path.clone();
    let built = tokio::task::spawn_blocking(move || zip_stream_manifest(&manifest_path)).await;
    cleanup_staged(&source.path, source.staged).await;

    match built {
        Ok(Ok(manifest)) => Ok(Json(manifest)),
        Ok(Err(ArchiveError::UnsupportedFormat)) => Err(stream_unsupported()),
        Ok(Err(error)) => Err(map_archive_error(error)),
        Err(error) => Err(ApiError::unprocessable(
            "stream_failed",
            format!("Manifest task failed: {error}"),
        )),
    }
}

/// A resolved single `Range` of one entry.
///
/// Shared with the extracted-cache file route (#231) so both transports apply
/// exactly the same `Range` rules.
pub(crate) struct EntryRange {
    pub(crate) start: u64,
    pub(crate) length: u64,
    pub(crate) partial: bool,
}

/// Resolves the `Range` header of an entry request against the entry size.
///
/// `Err` means the range is malformed or unsatisfiable: the caller answers 416
/// with `Content-Range: bytes */<size>`.
pub(crate) fn resolve_entry_range(raw: Option<&str>, size: u64) -> Result<EntryRange, ()> {
    let Some(raw) = raw else {
        return Ok(EntryRange {
            start: 0,
            length: size,
            partial: false,
        });
    };
    let parsed = parse_range(raw).ok_or(())?.ok_or(())?;
    match parsed {
        RequestRange::Offset(start) => {
            if start >= size {
                return Err(());
            }
            Ok(EntryRange {
                start,
                length: size - start,
                partial: true,
            })
        }
        RequestRange::Bounded { start, end } => {
            if start >= size || end < start {
                return Err(());
            }
            let end = end.min(size - 1);
            Ok(EntryRange {
                start,
                length: end - start + 1,
                partial: true,
            })
        }
        RequestRange::Suffix(length) => {
            if length == 0 || size == 0 {
                return Err(());
            }
            let start = size.saturating_sub(length);
            Ok(EntryRange {
                start,
                length: size - start,
                partial: true,
            })
        }
    }
}

/// Raw content of one ZIP entry, with single-Range support so an interrupted
/// entry resumes at its byte offset (direct seek for stored entries,
/// decompress-and-skip for DEFLATE/Zstd).
///
/// A compressed entry cannot be resumed past [`MAX_COMPRESSED_RESUME_SKIP`]:
/// the server answers `409 entry_resume_restart` instead of spending minutes
/// decompressing the prefix before the first body byte, and `restart=1`
/// streams the entry from 0 with the normal headers (#309).
///
/// The response is a plain byte stream; the client already knows the size and
/// CRC-32 from the manifest.
pub(crate) async fn entry(
    state: &AppState,
    game_id: i64,
    version: &VersionRow,
    index: usize,
    headers: &HeaderMap,
    restart: bool,
) -> Result<Response, ApiError> {
    let started = std::time::Instant::now();
    let source = resolve_stream_source(state, version).await?;
    let version_id = version.id;

    if source.format != ArchiveFormat::Zip {
        cleanup_staged(&source.path, source.staged).await;
        return Err(stream_unsupported());
    }

    let info_path = source.path.clone();
    let info = match tokio::task::spawn_blocking(move || zip_entry_info(&info_path, index)).await {
        Ok(Ok(info)) => info,
        Ok(Err(ArchiveError::EntryNotFound)) => {
            cleanup_staged(&source.path, source.staged).await;
            return Err(ApiError::not_found(
                "entry_not_found",
                "This archive does not have that entry",
            ));
        }
        Ok(Err(ArchiveError::UnsupportedFormat)) => {
            cleanup_staged(&source.path, source.staged).await;
            return Err(stream_unsupported());
        }
        Ok(Err(error)) => {
            cleanup_staged(&source.path, source.staged).await;
            return Err(map_archive_error(error));
        }
        Err(error) => {
            cleanup_staged(&source.path, source.staged).await;
            return Err(ApiError::unprocessable(
                "stream_failed",
                format!("Entry task failed: {error}"),
            ));
        }
    };

    // `restart=1` ignores the Range header by contract: the client asks for
    // the whole entry with the normal headers (#309).
    let range = if restart {
        EntryRange {
            start: 0,
            length: info.size,
            partial: false,
        }
    } else {
        let requested = headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok());
        match resolve_entry_range(requested, info.size) {
            Ok(range) => range,
            Err(()) => {
                cleanup_staged(&source.path, source.staged).await;
                let mut response = ApiError::new(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "invalid_range",
                    "The requested byte range cannot be satisfied",
                )
                .into_response();
                if let Ok(value) = HeaderValue::from_str(&format!("bytes */{}", info.size)) {
                    response.headers_mut().insert(header::CONTENT_RANGE, value);
                }
                response
                    .headers_mut()
                    .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
                return Ok(response);
            }
        }
    };

    if info.is_dir {
        cleanup_staged(&source.path, source.staged).await;
        return Err(ApiError::not_found(
            "entry_not_found",
            "Directories have no content",
        ));
    }

    // A compressed entry is walked from byte 0 to `offset` before the first
    // body byte. Past a few milliseconds of work that is worse than asking
    // the client to restart the entry, so say so instead of stalling (#309).
    if range.start > MAX_COMPRESSED_RESUME_SKIP {
        let check_path = source.path.clone();
        let stored = match tokio::task::spawn_blocking(move || {
            zip_entry_is_stored(&check_path, index)
        })
        .await
        {
            Ok(Ok(stored)) => stored,
            Ok(Err(error)) => {
                cleanup_staged(&source.path, source.staged).await;
                return Err(map_archive_error(error));
            }
            Err(error) => {
                cleanup_staged(&source.path, source.staged).await;
                return Err(ApiError::unprocessable(
                    "stream_failed",
                    format!("Entry task failed: {error}"),
                ));
            }
        };
        if !stored {
            cleanup_staged(&source.path, source.staged).await;
            tracing::info!(
                game_id,
                version_id,
                index,
                path = %info.path,
                start = range.start,
                threshold = MAX_COMPRESSED_RESUME_SKIP,
                "stream entry resume refused: compressed entry"
            );
            return Ok(resume_restart(&info, range.start));
        }
    }

    let file_name = info
        .path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("entry.bin");
    let content_type = mime_guess::from_path(&info.path).first_or_octet_stream();
    let mut builder = Response::builder()
        .status(if range.partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, range.length)
        .header(
            header::CACHE_CONTROL,
            // `no-transform` tells Caddy, nginx and other intermediaries that
            // the body must not be re-encoded (#310).
            HeaderValue::from_static("private, no-store, no-transform"),
        )
        // Never let nginx buffer an entry: a buffered multi-hundred-MB entry
        // looks like a stalled download to the client (#204). Caddy ignores
        // this header; `Incremental` below covers it.
        .header(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        )
        // RFC 10036 incremental forwarding: Caddy 2.11.7+ sends the response
        // head immediately instead of waiting for the first body byte to
        // decide on an encoding (#310).
        .header(
            HeaderName::from_static("incremental"),
            HeaderValue::from_static("?1"),
        );
    if let Ok(value) = HeaderValue::from_str(&content_disposition(file_name)) {
        builder = builder.header(header::CONTENT_DISPOSITION, value);
    }
    if range.partial {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{}",
                range.start,
                range.start + range.length - 1,
                info.size
            ),
        );
    }

    let cleanup = source.staged.then(|| source.path.clone());
    tracing::info!(
        game_id,
        version_id,
        index,
        path = %info.path,
        size = info.size,
        start = range.start,
        length = range.length,
        partial = range.partial,
        restart,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "stream entry response"
    );
    builder
        .body(entry_body(
            source.path,
            index,
            range.start,
            range.length,
            cleanup,
            game_id,
            version_id,
        ))
        .map_err(|_| ApiError::internal())
}

/// Decompression work a compressed-entry resume may cost before the server
/// refuses it (#309): 1 MiB is a few milliseconds of DEFLATE/Zstd, while the
/// cost grows linearly, so a 14.9 GiB offset means minutes without a body byte.
pub(crate) const MAX_COMPRESSED_RESUME_SKIP: u64 = 1024 * 1024;

/// Response header of the per-entry restart signal (#309).
pub(crate) const ENTRY_RESUME_HEADER: &str = "x-ludex-entry-resume";

/// `409 entry_resume_restart`: the entry is compressed and the requested
/// offset would require decompressing more than
/// [`MAX_COMPRESSED_RESUME_SKIP`] before the first body byte.
///
/// The body is the usual `{"error":{"code","message"}}` JSON. Both the code
/// and `X-Ludex-Entry-Resume: restart` tell the client to discard the entry's
/// partial file and re-request the same URL with `?restart=1`, which streams
/// the entry from byte 0 with a normal `200` and the full `Content-Length`.
fn resume_restart(info: &StreamEntry, offset: u64) -> Response {
    let mut response = ApiError::conflict(
        "entry_resume_restart",
        format!(
            "Entry {} is compressed and cannot resume at byte {offset}; restart it from byte 0 with ?restart=1",
            info.path
        ),
    )
    .into_response();
    response.headers_mut().insert(
        HeaderName::from_static(ENTRY_RESUME_HEADER),
        HeaderValue::from_static("restart"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
