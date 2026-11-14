//! Archive detection, verification and streaming extraction.
//!
//! Supports ZIP (including Zip64), RAR (RAR4/RAR5) and 7z. Extraction is
//! streaming: entries are copied in 64 KiB chunks and never loaded whole in
//! memory. Every entry path is validated before writing, so archives cannot
//! escape the destination directory.
//!
//! ```
//! use std::path::Path;
//! let format = ludex_core::archive::detect_format(Path::new("missing.zip"));
//! assert!(format.is_err());
//! ```

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sevenz_rust2::{Archive as SevenZipArchive, Password, SevenZArchiveEntry, SevenZReader};
use sha2::{Digest, Sha256};
use zip::read::{ZipArchive, ZipFile};
use zip::result::ZipError;

/// Maximum accepted length of an archive entry path, in bytes.
pub const MAX_ENTRY_PATH_LEN: usize = 1024;

/// Size of the copy buffer used during extraction.
const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// Progress is reported at most every mebibyte written.
const PROGRESS_STEP_BYTES: u64 = 1024 * 1024;

/// Archive container formats understood by Ludex.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// ZIP archive, including Zip64.
    Zip,
    /// RAR archive, RAR4 or RAR5.
    Rar,
    /// 7-Zip archive.
    SevenZip,
    /// Not a supported archive (or empty file).
    #[default]
    Unknown,
}

impl ArchiveFormat {
    /// Canonical lowercase extension, when the format is supported.
    pub fn extension(self) -> Option<&'static str> {
        match self {
            ArchiveFormat::Zip => Some("zip"),
            ArchiveFormat::Rar => Some("rar"),
            ArchiveFormat::SevenZip => Some("7z"),
            ArchiveFormat::Unknown => None,
        }
    }

    /// Human-readable label used in logs and errors.
    pub fn label(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::Rar => "rar",
            ArchiveFormat::SevenZip => "7z",
            ArchiveFormat::Unknown => "unknown",
        }
    }
}

/// Errors returned by the archive layer.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// Underlying filesystem error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The file is not a supported archive format.
    #[error("unsupported archive format")]
    UnsupportedFormat,
    /// The archive is corrupt or uses an unsupported feature.
    #[error("invalid archive: {0}")]
    InvalidArchive(String),
    /// The archive is encrypted and no password was supplied.
    #[error("archive is encrypted and requires a password")]
    PasswordRequired,
    /// None of the supplied passwords worked.
    #[error("wrong archive password")]
    WrongPassword,
    /// An entry path would escape the destination directory or is malformed.
    #[error("unsafe archive entry: {0}")]
    UnsafeEntry(String),
    /// The requested entry index is not in the archive.
    #[error("archive entry not found")]
    EntryNotFound,
}

impl ArchiveError {
    /// Returns `true` for password-related failures, which callers may retry
    /// with another password.
    pub fn is_password_error(&self) -> bool {
        matches!(
            self,
            ArchiveError::PasswordRequired | ArchiveError::WrongPassword
        )
    }
}

/// Result of [`verify_archive`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveReport {
    /// Detected container format.
    pub format: ArchiveFormat,
    /// Number of files (directories excluded).
    pub entry_count: usize,
    /// Sum of the uncompressed sizes declared by the archive.
    pub total_uncompressed: u64,
    /// Whether at least one entry (or the archive header) is encrypted.
    pub encrypted: bool,
}

/// Options controlling [`extract_archive`].
#[derive(Debug, Clone)]
pub struct ExtractOptions {
    /// Passwords tried in order for encrypted archives.
    pub passwords: Vec<String>,
    /// Overwrite files that already exist in the destination.
    pub overwrite: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            passwords: Vec::new(),
            overwrite: true,
        }
    }
}

impl ExtractOptions {
    /// Creates options with the given password list and overwrite enabled.
    pub fn with_passwords(passwords: Vec<String>) -> Self {
        Self {
            passwords,
            overwrite: true,
        }
    }
}

/// Progress snapshot emitted during extraction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractProgress {
    /// Number of files in the archive (directories excluded).
    pub entries_total: usize,
    /// Number of files fully written so far.
    pub entries_done: usize,
    /// Sum of the declared uncompressed sizes, in bytes.
    pub bytes_total: u64,
    /// Bytes written so far.
    pub bytes_done: u64,
    /// Entry currently being written.
    pub current_file: String,
}

/// Result of [`extract_archive`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractReport {
    /// Number of files written.
    pub files: usize,
    /// Number of bytes written.
    pub bytes: u64,
}

/// Reads the magic bytes of `path` and returns the detected format.
///
/// Only the first eight bytes are read.
pub fn detect_format(path: &Path) -> io::Result<ArchiveFormat> {
    let mut file = File::open(path)?;
    let mut magic = [0_u8; 8];
    let mut read = 0;
    while read < magic.len() {
        let count = file.read(&mut magic[read..])?;
        if count == 0 {
            break;
        }
        read += count;
    }
    Ok(format_from_magic(&magic[..read]))
}

fn format_from_magic(magic: &[u8]) -> ArchiveFormat {
    const ZIP_LOCAL: &[u8] = b"PK\x03\x04";
    const ZIP_EMPTY: &[u8] = b"PK\x05\x06";
    const ZIP_SPANNED: &[u8] = b"PK\x07\x08";
    const RAR: &[u8] = b"Rar!\x1a\x07";
    const SEVEN_ZIP: &[u8] = b"7z\xbc\xaf\x27\x1c";

    if magic.starts_with(ZIP_LOCAL)
        || magic.starts_with(ZIP_EMPTY)
        || magic.starts_with(ZIP_SPANNED)
    {
        ArchiveFormat::Zip
    } else if magic.starts_with(RAR) {
        ArchiveFormat::Rar
    } else if magic.starts_with(SEVEN_ZIP) {
        ArchiveFormat::SevenZip
    } else {
        ArchiveFormat::Unknown
    }
}

/// Returns `true` when the path has a `.zip`, `.rar` or `.7z` extension
/// (case-insensitive). This is a cheap pre-filter; use [`detect_format`] to be
/// sure.
pub fn is_archive_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "zip" | "rar" | "7z"
            )
        })
}

/// Maps a file name extension to the archive format Ludex can handle.
///
/// This is a pure name check used to describe a version before any byte is
/// read (#226): `.zip` → [`ArchiveFormat::Zip`], `.7z` →
/// [`ArchiveFormat::SevenZip`], `.rar` → [`ArchiveFormat::Rar`], anything else
/// (`.tar`, `.gz`, `.exe`, no extension …) → [`ArchiveFormat::Unknown`].
/// Case-insensitive; never touches the filesystem.
pub fn format_from_extension(path: &Path) -> ArchiveFormat {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("zip") => ArchiveFormat::Zip,
        Some("rar") => ArchiveFormat::Rar,
        Some("7z") => ArchiveFormat::SevenZip,
        _ => ArchiveFormat::Unknown,
    }
}

/// Opens an archive and reports its entries without extracting anything.
///
/// The password is only needed for archives with encrypted headers (RAR, 7z).
/// Per-entry encryption is reported through [`ArchiveReport::encrypted`].
pub fn verify_archive(path: &Path, password: Option<&str>) -> Result<ArchiveReport, ArchiveError> {
    match detect_format(path)? {
        ArchiveFormat::Zip => verify_zip(path),
        ArchiveFormat::Rar => verify_rar(path, password),
        ArchiveFormat::SevenZip => verify_seven_zip(path, password),
        ArchiveFormat::Unknown => Err(ArchiveError::UnsupportedFormat),
    }
}

fn verify_zip(path: &Path) -> Result<ArchiveReport, ArchiveError> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let mut report = ArchiveReport {
        format: ArchiveFormat::Zip,
        ..ArchiveReport::default()
    };
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        if entry.is_dir() {
            continue;
        }
        report.entry_count += 1;
        report.total_uncompressed = report.total_uncompressed.saturating_add(entry.size());
        report.encrypted |= entry.encrypted();
    }
    Ok(report)
}

fn verify_rar(path: &Path, password: Option<&str>) -> Result<ArchiveReport, ArchiveError> {
    ensure_usable_path(path)?;
    let archive = match password {
        Some(password) => unrar::Archive::with_password(path, password),
        None => unrar::Archive::new(path),
    };
    let listing = archive.open_for_listing().map_err(map_unrar_error)?;

    let mut report = ArchiveReport {
        format: ArchiveFormat::Rar,
        ..ArchiveReport::default()
    };
    for header in listing {
        let header = header.map_err(map_unrar_error)?;
        if header.is_directory() {
            continue;
        }
        report.entry_count += 1;
        report.total_uncompressed = report
            .total_uncompressed
            .saturating_add(header.unpacked_size);
        report.encrypted |= header.is_encrypted();
    }
    Ok(report)
}

fn verify_seven_zip(path: &Path, password: Option<&str>) -> Result<ArchiveReport, ArchiveError> {
    let archive = open_seven_zip_metadata(path, password)?;
    let mut report = ArchiveReport {
        format: ArchiveFormat::SevenZip,
        encrypted: seven_zip_is_encrypted(&archive),
        ..ArchiveReport::default()
    };
    for entry in &archive.files {
        if entry.is_directory() || entry.is_anti_item() {
            continue;
        }
        report.entry_count += 1;
        report.total_uncompressed = report.total_uncompressed.saturating_add(entry.size());
    }
    Ok(report)
}

fn open_seven_zip_metadata(
    path: &Path,
    password: Option<&str>,
) -> Result<SevenZipArchive, ArchiveError> {
    let result = match password {
        Some(password) => SevenZipArchive::open_with_password(path, &Password::from(password)),
        None => SevenZipArchive::open(path),
    };
    result.map_err(map_seven_zip_error)
}

fn seven_zip_is_encrypted(archive: &SevenZipArchive) -> bool {
    archive.folders.iter().any(|folder| {
        folder.coders.iter().any(|coder| {
            coder.decompression_method_id() == sevenz_rust2::SevenZMethod::ID_AES256SHA256
        })
    })
}

/// Extracts every entry of `path` into `dest`, streaming to disk.
///
/// The destination directory is created when missing. Passwords from
/// [`ExtractOptions::passwords`] are tried in order. Absolute paths, `..`,
/// drive letters, NUL bytes and link entries are rejected with
/// [`ArchiveError::UnsafeEntry`].
pub fn extract_archive(
    path: &Path,
    dest: &Path,
    opts: &ExtractOptions,
    progress: &mut dyn FnMut(ExtractProgress),
) -> Result<ExtractReport, ArchiveError> {
    ensure_usable_path(dest)?;
    let format = detect_format(path)?;
    if format == ArchiveFormat::Unknown {
        return Err(ArchiveError::UnsupportedFormat);
    }
    fs::create_dir_all(dest)?;
    match format {
        ArchiveFormat::Zip => extract_zip(path, dest, opts, progress),
        ArchiveFormat::Rar => extract_rar(path, dest, opts, progress),
        ArchiveFormat::SevenZip => extract_seven_zip(path, dest, opts, progress),
        ArchiveFormat::Unknown => Err(ArchiveError::UnsupportedFormat),
    }
}

/// Normalises and validates an archive entry path.
///
/// Returns a relative path with `/` separators, or [`ArchiveError::UnsafeEntry`]
/// when the entry is absolute, contains `..`, a drive letter, a NUL byte, a
/// colon (NTFS alternate data stream) or is longer than
/// [`MAX_ENTRY_PATH_LEN`].
pub fn sanitize_entry_path(name: &str) -> Result<PathBuf, ArchiveError> {
    if name.is_empty() {
        return Err(ArchiveError::UnsafeEntry("empty entry name".to_string()));
    }
    if name.len() > MAX_ENTRY_PATH_LEN {
        return Err(ArchiveError::UnsafeEntry(format!(
            "entry path longer than {MAX_ENTRY_PATH_LEN} bytes"
        )));
    }
    if name.contains('\0') {
        return Err(ArchiveError::UnsafeEntry(
            "entry path contains a NUL byte".to_string(),
        ));
    }

    let normalised = name.replace('\\', "/");
    if normalised.starts_with('/') {
        return Err(ArchiveError::UnsafeEntry(name.to_string()));
    }

    let mut safe = PathBuf::new();
    for segment in normalised.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(ArchiveError::UnsafeEntry(name.to_string()));
        }
        if segment.contains(':') {
            return Err(ArchiveError::UnsafeEntry(name.to_string()));
        }
        safe.push(segment);
    }

    if safe.as_os_str().is_empty() {
        return Err(ArchiveError::UnsafeEntry(name.to_string()));
    }
    Ok(safe)
}

fn extract_zip(
    path: &Path,
    dest: &Path,
    opts: &ExtractOptions,
    progress: &mut dyn FnMut(ExtractProgress),
) -> Result<ExtractReport, ArchiveError> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;

    let mut entries_total = 0;
    let mut bytes_total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        if entry.is_dir() {
            continue;
        }
        entries_total += 1;
        bytes_total = bytes_total.saturating_add(entry.size());
    }

    let mut sink = Sink::new(progress, entries_total, bytes_total);
    let mut report = ExtractReport::default();

    for index in 0..archive.len() {
        let mut entry = open_zip_entry(&mut archive, index, opts)?;
        let name = entry.name().to_string();
        let relative = sanitize_entry_path(&name)?;

        if entry.is_dir() {
            fs::create_dir_all(dest.join(relative))?;
            continue;
        }
        if entry.is_symlink() {
            return Err(ArchiveError::UnsafeEntry(format!("link entry: {name}")));
        }

        let out_path = dest.join(&relative);
        ensure_within(dest, &out_path, &name)?;
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !opts.overwrite && out_path.exists() {
            sink.skip_entry(&name);
            continue;
        }

        sink.begin_entry(&name);
        let mut writer = BufWriter::new(File::create(&out_path)?);
        let written = copy_with_progress(&mut entry, &mut writer, &mut sink)?;
        writer.flush()?;
        report.files += 1;
        report.bytes = report.bytes.saturating_add(written);
        sink.finish_entry();
    }

    Ok(report)
}

fn open_zip_entry<'a>(
    archive: &'a mut ZipArchive<BufReader<File>>,
    index: usize,
    opts: &ExtractOptions,
) -> Result<ZipFile<'a, BufReader<File>>, ArchiveError> {
    // Inspect the raw metadata first: asking for the entry with a password is
    // only possible when the archive borrow is free again.
    let encrypted = {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        entry.encrypted()
    };

    if !encrypted {
        return archive.by_index(index).map_err(map_zip_error);
    }

    // Probe each password on a short read so that a wrong password does not
    // silently produce garbage, then reopen the entry once with the winner.
    let mut chosen: Option<&str> = None;
    for password in &opts.passwords {
        match archive.by_index_decrypt(index, password.as_bytes()) {
            Ok(mut entry) => {
                let mut probe = [0_u8; 1];
                let readable = entry.read(&mut probe).is_ok();
                drop(entry);
                if readable {
                    chosen = Some(password.as_str());
                    break;
                }
            }
            Err(ZipError::InvalidPassword) => continue,
            Err(ZipError::UnsupportedArchive(_)) => continue,
            Err(error) => return Err(map_zip_error(error)),
        }
    }

    let Some(password) = chosen else {
        return Err(ArchiveError::PasswordRequired);
    };
    archive
        .by_index_decrypt(index, password.as_bytes())
        .map_err(map_zip_error)
}

fn extract_rar(
    path: &Path,
    dest: &Path,
    opts: &ExtractOptions,
    progress: &mut dyn FnMut(ExtractProgress),
) -> Result<ExtractReport, ArchiveError> {
    ensure_usable_path(path)?;
    let (entries_total, bytes_total) = rar_totals(path, opts)?;
    let mut sink = Sink::new(progress, entries_total, bytes_total);

    let mut last_password_error: Option<ArchiveError> = None;
    for password in password_candidates(opts) {
        sink.reset();
        match extract_rar_pass(path, dest, password, opts, &mut sink) {
            Ok(report) => return Ok(report),
            Err(error) if error.is_password_error() => last_password_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_password_error.unwrap_or(ArchiveError::PasswordRequired))
}

fn rar_totals(path: &Path, opts: &ExtractOptions) -> Result<(usize, u64), ArchiveError> {
    let mut last_password_error: Option<ArchiveError> = None;
    for password in password_candidates(opts) {
        match rar_listing(path, password) {
            Ok(totals) => return Ok(totals),
            Err(error) if error.is_password_error() => last_password_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_password_error.unwrap_or(ArchiveError::PasswordRequired))
}

fn rar_listing(path: &Path, password: Option<&str>) -> Result<(usize, u64), ArchiveError> {
    let archive = match password {
        Some(password) => unrar::Archive::with_password(path, password),
        None => unrar::Archive::new(path),
    };
    let listing = archive.open_for_listing().map_err(map_unrar_error)?;

    let mut files = 0;
    let mut bytes = 0_u64;
    for header in listing {
        let header = header.map_err(map_unrar_error)?;
        if header.is_directory() {
            continue;
        }
        files += 1;
        bytes = bytes.saturating_add(header.unpacked_size);
    }
    Ok((files, bytes))
}

fn extract_rar_pass(
    path: &Path,
    dest: &Path,
    password: Option<&str>,
    opts: &ExtractOptions,
    sink: &mut Sink<'_>,
) -> Result<ExtractReport, ArchiveError> {
    let archive = match password {
        Some(password) => unrar::Archive::with_password(path, password),
        None => unrar::Archive::new(path),
    };
    let mut archive = archive.open_for_processing().map_err(map_unrar_error)?;
    let mut report = ExtractReport::default();

    while let Some(header) = archive.read_header().map_err(map_unrar_error)? {
        let (name, is_directory, is_link, is_encrypted, size) = {
            let entry = header.entry();
            (
                entry.filename.to_string_lossy().to_string(),
                entry.is_directory(),
                is_link_attr(entry.file_attr),
                entry.is_encrypted(),
                entry.unpacked_size,
            )
        };

        if is_directory {
            let relative = sanitize_entry_path(&name)?;
            fs::create_dir_all(dest.join(relative))?;
            archive = header.skip().map_err(map_unrar_error)?;
            continue;
        }
        if is_link {
            return Err(ArchiveError::UnsafeEntry(format!("link entry: {name}")));
        }

        let relative = sanitize_entry_path(&name)?;
        let out_path = dest.join(&relative);
        ensure_within(dest, &out_path, &name)?;
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !opts.overwrite && out_path.exists() {
            archive = header.skip().map_err(map_unrar_error)?;
            sink.skip_entry(&name);
            continue;
        }

        sink.begin_entry(&name);
        archive = header
            .extract_to(&out_path)
            .map_err(|error| map_unrar_extract_error(error, is_encrypted))?;
        report.files += 1;
        report.bytes = report.bytes.saturating_add(size);
        sink.add_bytes(size);
        sink.finish_entry();
    }

    Ok(report)
}

fn extract_seven_zip(
    path: &Path,
    dest: &Path,
    opts: &ExtractOptions,
    progress: &mut dyn FnMut(ExtractProgress),
) -> Result<ExtractReport, ArchiveError> {
    let metadata = open_seven_zip_for_extraction(path, opts)?;
    let encrypted = seven_zip_is_encrypted(&metadata);
    let mut entries_total = 0;
    let mut bytes_total = 0_u64;
    for entry in &metadata.files {
        if entry.is_directory() || entry.is_anti_item() {
            continue;
        }
        entries_total += 1;
        bytes_total = bytes_total.saturating_add(entry.size());
    }

    let mut sink = Sink::new(progress, entries_total, bytes_total);
    let mut last_password_error: Option<ArchiveError> = None;

    for password in password_candidates(opts) {
        sink.reset();
        let reader_result = match password {
            Some(password) => SevenZReader::open(path, Password::from(password)),
            None => SevenZReader::open(path, Password::empty()),
        };
        let mut reader = match reader_result {
            Ok(reader) => reader,
            Err(error) => {
                let error = map_seven_zip_error(error);
                if error.is_password_error() {
                    last_password_error = Some(error);
                    continue;
                }
                return Err(error);
            }
        };

        let mut report = ExtractReport::default();
        let mut failure: Option<ArchiveError> = None;
        let outcome = reader.for_each_entries(|entry, entry_reader| {
            if failure.is_some() {
                return Ok(false);
            }
            match extract_seven_zip_entry(entry, entry_reader, dest, opts, &mut sink, &mut report) {
                Ok(()) => Ok(true),
                Err(error) => {
                    failure = Some(error);
                    Ok(false)
                }
            }
        });

        match (outcome, failure) {
            (_, Some(error)) => {
                let error = normalize_seven_zip_error(error, encrypted, password.is_some());
                if error.is_password_error() {
                    last_password_error = Some(error);
                    continue;
                }
                return Err(error);
            }
            (Err(error), None) => {
                let error = normalize_seven_zip_error(
                    map_seven_zip_error(error),
                    encrypted,
                    password.is_some(),
                );
                if error.is_password_error() {
                    last_password_error = Some(error);
                    continue;
                }
                return Err(error);
            }
            (Ok(()), None) => return Ok(report),
        }
    }

    Err(last_password_error.unwrap_or(ArchiveError::PasswordRequired))
}

/// Opens 7z metadata with the first password that works.
fn open_seven_zip_for_extraction(
    path: &Path,
    opts: &ExtractOptions,
) -> Result<SevenZipArchive, ArchiveError> {
    let mut last_password_error: Option<ArchiveError> = None;
    for password in password_candidates(opts) {
        match open_seven_zip_metadata(path, password) {
            Ok(archive) => return Ok(archive),
            Err(error) if error.is_password_error() => last_password_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_password_error.unwrap_or(ArchiveError::PasswordRequired))
}

fn extract_seven_zip_entry(
    entry: &SevenZArchiveEntry,
    reader: &mut dyn Read,
    dest: &Path,
    opts: &ExtractOptions,
    sink: &mut Sink<'_>,
    report: &mut ExtractReport,
) -> Result<(), ArchiveError> {
    let name = entry.name().to_string();

    if entry.is_directory() {
        // Some writers emit a root directory entry with an empty name.
        if name.is_empty() {
            return Ok(());
        }
        let relative = sanitize_entry_path(&name)?;
        fs::create_dir_all(dest.join(relative))?;
        return Ok(());
    }
    if entry.is_anti_item() {
        return Ok(());
    }
    if is_link_attr(entry.windows_attributes) {
        return Err(ArchiveError::UnsafeEntry(format!("link entry: {name}")));
    }

    let relative = sanitize_entry_path(&name)?;
    let out_path = dest.join(&relative);
    ensure_within(dest, &out_path, &name)?;
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !opts.overwrite && out_path.exists() {
        sink.skip_entry(&name);
        return Ok(());
    }

    sink.begin_entry(&name);
    let mut writer = BufWriter::new(File::create(&out_path)?);
    let written = copy_with_progress(reader, &mut writer, sink)?;
    writer.flush()?;
    report.files += 1;
    report.bytes = report.bytes.saturating_add(written);
    sink.finish_entry();
    Ok(())
}

/// Password candidates in the order they are tried: no password first, then
/// the user-supplied list.
fn password_candidates(opts: &ExtractOptions) -> Vec<Option<&str>> {
    let mut candidates = Vec::with_capacity(opts.passwords.len() + 1);
    candidates.push(None);
    for password in &opts.passwords {
        candidates.push(Some(password.as_str()));
    }
    candidates
}

/// Copies `reader` into `writer`, reporting progress per 64 KiB chunk.
fn copy_with_progress(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    sink: &mut Sink<'_>,
) -> Result<u64, ArchiveError> {
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        total = total.saturating_add(read as u64);
        sink.add_bytes(read as u64);
    }
    Ok(total)
}

/// Returns an error when `out_path` is not inside `dest`.
fn ensure_within(dest: &Path, out_path: &Path, name: &str) -> Result<(), ArchiveError> {
    if out_path.starts_with(dest) {
        Ok(())
    } else {
        Err(ArchiveError::UnsafeEntry(name.to_string()))
    }
}

/// Guards against NUL bytes in paths handed to the unrar FFI, which panics on
/// them.
fn ensure_usable_path(path: &Path) -> Result<(), ArchiveError> {
    if path.to_string_lossy().contains('\0') {
        return Err(ArchiveError::UnsafeEntry(
            "path contains a NUL byte".to_string(),
        ));
    }
    Ok(())
}

/// Detects symlink/hardlink entries from RAR/7z attribute fields.
fn is_link_attr(attributes: u32) -> bool {
    let unix_type = attributes & 0o170_000;
    if unix_type != 0 {
        unix_type == 0o120_000
    } else {
        attributes & 0x400 != 0
    }
}

/// Buffered progress reporter shared by all backends.
struct Sink<'a> {
    callback: &'a mut dyn FnMut(ExtractProgress),
    progress: ExtractProgress,
    last_emitted_bytes: u64,
}

impl<'a> Sink<'a> {
    fn new(
        callback: &'a mut dyn FnMut(ExtractProgress),
        entries_total: usize,
        bytes_total: u64,
    ) -> Self {
        let mut sink = Self {
            callback,
            progress: ExtractProgress {
                entries_total,
                entries_done: 0,
                bytes_total,
                bytes_done: 0,
                current_file: String::new(),
            },
            last_emitted_bytes: 0,
        };
        sink.emit();
        sink
    }

    fn reset(&mut self) {
        self.progress.entries_done = 0;
        self.progress.bytes_done = 0;
        self.progress.current_file.clear();
        self.last_emitted_bytes = 0;
        self.emit();
    }

    fn begin_entry(&mut self, name: &str) {
        self.progress.current_file.clear();
        self.progress.current_file.push_str(name);
        self.emit();
    }

    fn add_bytes(&mut self, bytes: u64) {
        self.progress.bytes_done = self.progress.bytes_done.saturating_add(bytes);
        if self
            .progress
            .bytes_done
            .saturating_sub(self.last_emitted_bytes)
            >= PROGRESS_STEP_BYTES
        {
            self.emit();
        }
    }

    fn skip_entry(&mut self, name: &str) {
        self.progress.current_file.clear();
        self.progress.current_file.push_str(name);
        self.progress.entries_done += 1;
        self.emit();
    }

    fn finish_entry(&mut self) {
        self.progress.entries_done += 1;
        self.emit();
    }

    fn emit(&mut self) {
        self.last_emitted_bytes = self.progress.bytes_done;
        (self.callback)(self.progress.clone());
    }
}

fn map_zip_error(error: ZipError) -> ArchiveError {
    match error {
        ZipError::Io(io_error) => ArchiveError::Io(io_error),
        ZipError::InvalidPassword => ArchiveError::WrongPassword,
        ZipError::UnsupportedArchive(reason) if reason == ZipError::PASSWORD_REQUIRED => {
            ArchiveError::PasswordRequired
        }
        ZipError::UnsupportedArchive(reason) => ArchiveError::InvalidArchive(reason.to_string()),
        ZipError::InvalidArchive(message) => ArchiveError::InvalidArchive(message.to_string()),
        ZipError::FileNotFound => ArchiveError::EntryNotFound,
        other => ArchiveError::InvalidArchive(other.to_string()),
    }
}

fn map_unrar_error(error: unrar::error::UnrarError) -> ArchiveError {
    use unrar::error::Code;

    match error.code {
        Code::MissingPassword => ArchiveError::PasswordRequired,
        Code::BadPassword => ArchiveError::WrongPassword,
        Code::BadArchive | Code::BadData | Code::UnknownFormat => {
            ArchiveError::InvalidArchive(error.to_string())
        }
        _ => ArchiveError::Io(io::Error::other(error.to_string())),
    }
}

/// A CRC failure on an encrypted entry usually means the password was wrong
/// (libunrar reports bad data instead of a bad password). Map it so the caller
/// can try the next password in the list.
fn map_unrar_extract_error(error: unrar::error::UnrarError, encrypted_entry: bool) -> ArchiveError {
    if encrypted_entry && error.code == unrar::error::Code::BadData {
        return ArchiveError::WrongPassword;
    }
    map_unrar_error(error)
}

fn map_seven_zip_error(error: sevenz_rust2::Error) -> ArchiveError {
    use sevenz_rust2::Error as SevenZipError;

    match error {
        SevenZipError::PasswordRequired => ArchiveError::PasswordRequired,
        SevenZipError::MaybeBadPassword(_) => ArchiveError::WrongPassword,
        SevenZipError::Io(io_error, _) => ArchiveError::Io(io_error),
        other => ArchiveError::InvalidArchive(other.to_string()),
    }
}

/// A decompression failure in an AES-encrypted 7z almost always means the
/// password was wrong. Reclassify it so the next password can be tried, while
/// leaving disk write errors untouched.
fn normalize_seven_zip_error(
    error: ArchiveError,
    encrypted: bool,
    supplied_password: bool,
) -> ArchiveError {
    let decompression_failure = matches!(
        &error,
        ArchiveError::Io(io_error)
            if matches!(
                io_error.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData
            )
    );
    if encrypted && decompression_failure {
        return if supplied_password {
            ArchiveError::WrongPassword
        } else {
            ArchiveError::PasswordRequired
        };
    }
    error
}

// ---------------------------------------------------------------------------
// Streaming install (archive -> uncompressed tar)
// ---------------------------------------------------------------------------

/// Tar block size, in bytes.
const TAR_BLOCK: u64 = 512;
/// Two zero blocks terminate a tar archive.
const TAR_END_BLOCKS: u64 = 1024;
/// Size of the name field in a GNU header.
const TAR_NAME_FIELD: usize = 100;
/// Pseudo path used by GNU tar for long-name entries.
const TAR_LONG_NAME_PATH: &str = "././@LongLink";

/// Result of [`stream_archive_as_tar`] or [`stream_zip_as_tar`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TarStreamReport {
    /// Number of file entries written.
    pub files: u64,
    /// Sum of the uncompressed file sizes, in bytes.
    pub bytes: u64,
    /// Exact number of tar bytes written (including headers and end blocks).
    pub tar_bytes: u64,
}

/// Computes the exact size of the tar stream [`stream_archive_as_tar`] would
/// produce for `path`.
///
/// Supports ZIP, 7z and RAR (RAR4/RAR5). No entry data is decompressed, so the
/// returned length is derived from the entry plan shared with the stream
/// writer. Encrypted archives are rejected with
/// [`ArchiveError::PasswordRequired`].
pub fn archive_tar_size(path: &Path) -> Result<u64, ArchiveError> {
    match detect_format(path)? {
        ArchiveFormat::Zip => zip_tar_size(path),
        ArchiveFormat::Rar => rar_tar_size(path),
        ArchiveFormat::SevenZip => seven_zip_tar_size(path),
        ArchiveFormat::Unknown => Err(ArchiveError::UnsupportedFormat),
    }
}

/// Streams `path` as an uncompressed tar archive to `writer`.
///
/// Supports ZIP (including Zip64 and zstd-compressed entries), 7z and RAR
/// (RAR4/RAR5). Entry paths are sanitised with [`sanitize_entry_path`], link
/// and special entries are rejected, and encrypted archives return
/// [`ArchiveError::PasswordRequired`] because this API has no password
/// parameter. Entry data is copied in chunks (never loaded whole in memory);
/// RAR entries are decompressed by libunrar into a private scratch file first,
/// because libunrar can only extract to a path.
pub fn stream_archive_as_tar<W: Write>(
    path: &Path,
    writer: &mut W,
) -> Result<TarStreamReport, ArchiveError> {
    match detect_format(path)? {
        ArchiveFormat::Zip => stream_zip_as_tar(path, writer),
        ArchiveFormat::Rar => stream_rar_as_tar(path, writer),
        ArchiveFormat::SevenZip => stream_seven_zip_as_tar(path, writer),
        ArchiveFormat::Unknown => Err(ArchiveError::UnsupportedFormat),
    }
}

/// Exact tar size for a ZIP archive, without decompressing any entry.
///
/// Returns [`ArchiveError::UnsupportedFormat`] for non-ZIP files,
/// [`ArchiveError::PasswordRequired`] for encrypted entries and
/// [`ArchiveError::UnsafeEntry`] for traversal or link entries.
pub fn zip_tar_size(path: &Path) -> Result<u64, ArchiveError> {
    if detect_format(path)? != ArchiveFormat::Zip {
        return Err(ArchiveError::UnsupportedFormat);
    }
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let entries = collect_zip_tar_entries(&mut archive)?;
    Ok(tar_total_len(entries.iter().map(|item| &item.entry)))
}

/// Streams a ZIP archive as an uncompressed tar to `writer`.
///
/// Encrypted ZIPs are rejected with [`ArchiveError::PasswordRequired`] because
/// this API has no password parameter. The returned [`TarStreamReport::tar_bytes`]
/// equals [`zip_tar_size`] for a valid archive.
pub fn stream_zip_as_tar<W: Write>(
    path: &Path,
    writer: &mut W,
) -> Result<TarStreamReport, ArchiveError> {
    if detect_format(path)? != ArchiveFormat::Zip {
        return Err(ArchiveError::UnsupportedFormat);
    }
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let entries = collect_zip_tar_entries(&mut archive)?;

    let mut counter = ByteCounter::new(writer);
    let mut report = TarStreamReport::default();
    {
        let mut builder = tar::Builder::new(&mut counter);
        for item in &entries {
            let mut data = archive.by_index(item.index).map_err(map_zip_error)?;
            write_tar_entry(&mut builder, &item.entry, &mut data)?;
            report.files += 1;
            report.bytes = report.bytes.saturating_add(item.entry.size);
        }
        builder.finish()?;
    }
    counter.flush()?;
    report.tar_bytes = counter.count();
    Ok(report)
}

/// Exact tar size for a RAR (RAR4/RAR5) archive, without decompressing any
/// entry.
///
/// Returns [`ArchiveError::UnsupportedFormat`] for non-RAR files,
/// [`ArchiveError::PasswordRequired`] for encrypted archives and
/// [`ArchiveError::UnsafeEntry`] for traversal, link or special entries.
pub fn rar_tar_size(path: &Path) -> Result<u64, ArchiveError> {
    if detect_format(path)? != ArchiveFormat::Rar {
        return Err(ArchiveError::UnsupportedFormat);
    }
    let entries = collect_rar_tar_entries(path)?;
    Ok(tar_total_len(entries.iter()))
}

/// Streams a RAR (RAR4/RAR5) archive as an uncompressed tar to `writer`.
///
/// Encrypted archives are rejected with [`ArchiveError::PasswordRequired`]
/// because this API has no password parameter. libunrar can only decompress to
/// a path, so each entry is materialised in a private scratch file, copied to
/// `writer` in 64 KiB chunks and then deleted: memory stays bounded and no
/// temporary file outlives the call. The returned [`TarStreamReport::tar_bytes`]
/// equals [`rar_tar_size`] for a valid archive.
pub fn stream_rar_as_tar<W: Write>(
    path: &Path,
    writer: &mut W,
) -> Result<TarStreamReport, ArchiveError> {
    stream_rar_as_tar_in(path, writer, &std::env::temp_dir())
}

fn stream_rar_as_tar_in<W: Write>(
    path: &Path,
    writer: &mut W,
    scratch_base: &Path,
) -> Result<TarStreamReport, ArchiveError> {
    if detect_format(path)? != ArchiveFormat::Rar {
        return Err(ArchiveError::UnsupportedFormat);
    }
    let entries = collect_rar_tar_entries(path)?;
    let expected_tar_bytes = tar_total_len(entries.iter());

    let mut scratch = ScratchSpace::new(scratch_base)?;
    let mut archive = unrar::Archive::new(path)
        .open_for_processing()
        .map_err(map_unrar_error)?;
    let mut planned = entries.iter();
    let mut counter = ByteCounter::new(writer);
    let mut report = TarStreamReport::default();
    {
        let mut builder = tar::Builder::new(&mut counter);
        while let Some(header) = archive.read_header().map_err(map_unrar_error)? {
            let (name, size, is_directory, encrypted) = {
                let entry = header.entry();
                (
                    entry.filename.to_string_lossy().to_string(),
                    entry.unpacked_size,
                    entry.is_directory(),
                    entry.is_encrypted(),
                )
            };
            if is_directory {
                archive = header.skip().map_err(map_unrar_error)?;
                continue;
            }
            if encrypted {
                return Err(ArchiveError::PasswordRequired);
            }

            let Some(expected) = planned.next() else {
                return Err(ArchiveError::InvalidArchive(
                    "RAR entry set changed while streaming".to_string(),
                ));
            };
            let actual_name = tar_name(&sanitize_entry_path(&name)?);
            if actual_name != expected.name || size != expected.size {
                return Err(ArchiveError::InvalidArchive(format!(
                    "RAR entry changed while streaming: {actual_name}"
                )));
            }

            let scratch_path = scratch.next_file();
            archive = header.extract_to(&scratch_path).map_err(map_unrar_error)?;
            let mut data = File::open(&scratch_path)?;
            write_tar_entry(&mut builder, expected, &mut data)?;
            // Keep disk use bounded to a single entry while streaming.
            let _ = fs::remove_file(&scratch_path);

            report.files += 1;
            report.bytes = report.bytes.saturating_add(expected.size);
        }
        if planned.next().is_some() {
            return Err(ArchiveError::InvalidArchive(
                "RAR entry set changed while streaming".to_string(),
            ));
        }
        builder.finish()?;
    }
    counter.flush()?;
    report.tar_bytes = counter.count();

    // Guards against libunrar yielding a different entry set than the listing,
    // which would break the advertised Content-Length.
    if report.tar_bytes != expected_tar_bytes {
        return Err(ArchiveError::InvalidArchive(
            "RAR tar size changed while streaming".to_string(),
        ));
    }
    Ok(report)
}

/// Unix file type bits from a RAR attribute field, when the archive recorded a
/// Unix mode (RAR stores Unix attributes for archives created on Unix).
fn rar_unix_file_type(attributes: u32) -> Option<u32> {
    let file_type = attributes & 0o170_000;
    (file_type != 0).then_some(file_type)
}

/// Octal file type for a Unix regular file.
const RAR_UNIX_REGULAR_FILE: u32 = 0o100_000;

/// Selects and validates the RAR entries that will be streamed.
///
/// Only regular files are accepted: directories are skipped, links and special
/// files (FIFO, socket, device) are rejected, and encrypted archives are
/// reported before any data is written.
fn collect_rar_tar_entries(path: &Path) -> Result<Vec<TarEntry>, ArchiveError> {
    ensure_usable_path(path)?;
    let listing = unrar::Archive::new(path)
        .open_for_listing()
        .map_err(map_unrar_error)?;

    let mut entries = Vec::new();
    for header in listing {
        let header = header.map_err(map_unrar_error)?;
        if header.is_directory() {
            continue;
        }
        if header.is_encrypted() {
            return Err(ArchiveError::PasswordRequired);
        }
        if header.is_split() {
            return Err(ArchiveError::InvalidArchive(
                "multi-volume RAR archives cannot be streamed".to_string(),
            ));
        }

        let name = header.filename.to_string_lossy().to_string();
        let attributes = header.file_attr;
        match rar_unix_file_type(attributes) {
            Some(file_type) if file_type != RAR_UNIX_REGULAR_FILE => {
                return Err(ArchiveError::UnsafeEntry(format!(
                    "link or special entry: {name}"
                )));
            }
            Some(_) => {}
            None if is_link_attr(attributes) => {
                return Err(ArchiveError::UnsafeEntry(format!("link entry: {name}")));
            }
            None => {}
        }

        let relative = sanitize_entry_path(&name)?;
        let name = tar_name(&relative);
        let mode = tar_mode(&name, rar_unix_mode(attributes));
        entries.push(TarEntry {
            name,
            size: header.unpacked_size,
            mode,
        });
    }
    Ok(entries)
}

/// Unix permission bits stored in a RAR attribute field, when present.
fn rar_unix_mode(attributes: u32) -> Option<u32> {
    rar_unix_file_type(attributes).map(|_| attributes & 0o777)
}

/// Per-call scratch directory bridging libunrar's path-only extraction to the
/// tar writer. The directory is removed when the value is dropped, including on
/// error, and each entry file is deleted as soon as it has been copied.
struct ScratchSpace {
    dir: PathBuf,
    next: u64,
}

impl ScratchSpace {
    fn new(base: &Path) -> Result<Self, ArchiveError> {
        for attempt in 0..100_u32 {
            let dir = base.join(format!("ludex-rar-{}-{attempt}", std::process::id()));
            match create_scratch_dir(&dir) {
                Ok(()) => return Ok(Self { dir, next: 0 }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ArchiveError::Io(io::Error::other(
            "could not create a scratch directory for RAR streaming",
        )))
    }

    fn next_file(&mut self) -> PathBuf {
        self.next += 1;
        self.dir.join(format!("entry-{}", self.next))
    }
}

/// Creates the scratch directory, owner-only on Unix: extracted entries must
/// not be readable by other local users while they are being streamed.
fn create_scratch_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

impl Drop for ScratchSpace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// Resumable, per-entry stream installs (#115)
// ---------------------------------------------------------------------------

/// One entry of a [`StreamManifest`].
///
/// `index` is the position in the archive's central directory: it is stable
/// for a given archive and is what the entry endpoint expects. `path` is a
/// sanitised relative path with `/` separators, so clients can join it to a
/// staging folder without re-validating it (they should still do it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamEntry {
    /// Position in the archive central directory.
    pub index: usize,
    /// Sanitised relative path, `/` separated.
    pub path: String,
    /// Uncompressed size in bytes (`0` for directories).
    pub size: u64,
    /// Compressed size in bytes (`0` for directories).
    pub compressed_size: u64,
    /// Permission bits: the archive's Unix mode, `0o755` for scripts and
    /// AppImages, `0o644` otherwise.
    pub mode: u32,
    /// CRC-32 of the uncompressed data (`0` for directories).
    pub crc32: u32,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

/// Ordered entry list for a resumable stream install.
///
/// The manifest is generated from the ZIP central directory, in central
/// directory order, and contains both files and directories. Clients download
/// the files they do not have yet and create the directories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamManifest {
    /// Container format. `zip` today; 7z/RAR remain sequential tar streams.
    pub format: String,
    /// Number of entries, directories included.
    pub entry_count: usize,
    /// Sum of the uncompressed file sizes (directories excluded).
    pub total_bytes: u64,
    /// Sum of the compressed file sizes (directories excluded).
    pub total_compressed_bytes: u64,
    /// Entries in central-directory order.
    pub entries: Vec<StreamEntry>,
}

impl StreamManifest {
    /// Stable fingerprint of the manifest contents.
    ///
    /// The desktop uses it to decide whether a persisted checkpoint still
    /// matches the archive the server would serve now: when the archive was
    /// replaced under the same version row, entry indexes and sizes may have
    /// changed and the checkpoint must be discarded.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        let digest = Sha256::digest(&encoded);
        hex::encode(digest)
    }
}

/// Builds the ordered [`StreamManifest`] of a ZIP archive.
///
/// Reads only the central directory: no entry is decompressed. Encrypted and
/// symlink entries are rejected, exactly like the tar stream, so a client that
/// falls back to `/stream` sees the same failure.
pub fn zip_stream_manifest(path: &Path) -> Result<StreamManifest, ArchiveError> {
    if detect_format(path)? != ArchiveFormat::Zip {
        return Err(ArchiveError::UnsupportedFormat);
    }
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;

    let mut manifest = StreamManifest {
        format: ArchiveFormat::Zip.label().to_string(),
        entry_count: 0,
        total_bytes: 0,
        total_compressed_bytes: 0,
        entries: Vec::with_capacity(archive.len()),
    };

    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        if entry.encrypted() {
            return Err(ArchiveError::PasswordRequired);
        }
        if entry.is_symlink() {
            return Err(ArchiveError::UnsafeEntry(format!(
                "link entry: {}",
                entry.name()
            )));
        }

        let is_dir = entry.is_dir();
        let relative = match sanitize_entry_path(entry.name()) {
            Ok(relative) => relative,
            // Root directory markers such as "./" carry no path; they are not
            // needed by the client and are skipped rather than rejected.
            Err(_) if is_dir => continue,
            Err(error) => return Err(error),
        };
        let name = tar_name(&relative);
        let mode = tar_mode(&name, entry.unix_mode());
        let size = if is_dir { 0 } else { entry.size() };
        let compressed_size = if is_dir { 0 } else { entry.compressed_size() };
        let crc32 = if is_dir { 0 } else { entry.crc32() };

        manifest.total_bytes = manifest.total_bytes.saturating_add(size);
        manifest.total_compressed_bytes = manifest
            .total_compressed_bytes
            .saturating_add(compressed_size);
        manifest.entries.push(StreamEntry {
            index,
            path: name,
            size,
            compressed_size,
            mode,
            crc32,
            is_dir,
        });
    }

    manifest.entry_count = manifest.entries.len();
    Ok(manifest)
}

/// Reads the metadata of one ZIP entry, without decompressing it.
///
/// Returns [`ArchiveError::InvalidArchive`] when `index` is out of range.
pub fn zip_entry_info(path: &Path, index: usize) -> Result<StreamEntry, ArchiveError> {
    if detect_format(path)? != ArchiveFormat::Zip {
        return Err(ArchiveError::UnsupportedFormat);
    }
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
    if entry.encrypted() {
        return Err(ArchiveError::PasswordRequired);
    }
    if entry.is_symlink() {
        return Err(ArchiveError::UnsafeEntry(format!(
            "link entry: {}",
            entry.name()
        )));
    }
    let is_dir = entry.is_dir();
    let relative = sanitize_entry_path(entry.name())?;
    let name = tar_name(&relative);
    let mode = tar_mode(&name, entry.unix_mode());
    Ok(StreamEntry {
        index,
        path: name,
        size: if is_dir { 0 } else { entry.size() },
        compressed_size: if is_dir { 0 } else { entry.compressed_size() },
        mode,
        crc32: if is_dir { 0 } else { entry.crc32() },
        is_dir,
    })
}

/// Writes `length` bytes of one ZIP entry starting at uncompressed `offset`.
///
/// Stored (uncompressed) entries seek straight to `data_start + offset`, so a
/// ranged resume costs one seek. Compressed entries are decompressed from the
/// start and the first `offset` bytes are skipped; that keeps memory bounded
/// but makes a resume of a large DEFLATE/Zstd entry proportional to its offset.
/// Returns the number of bytes written (`0` for directories).
pub fn write_zip_entry_range<W: Write>(
    path: &Path,
    index: usize,
    offset: u64,
    length: Option<u64>,
    writer: &mut W,
) -> Result<u64, ArchiveError> {
    if detect_format(path)? != ArchiveFormat::Zip {
        return Err(ArchiveError::UnsupportedFormat);
    }
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
    let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
    if entry.encrypted() {
        return Err(ArchiveError::PasswordRequired);
    }
    if entry.is_symlink() {
        return Err(ArchiveError::UnsafeEntry(format!(
            "link entry: {}",
            entry.name()
        )));
    }
    if entry.is_dir() {
        return Ok(0);
    }
    let size = entry.size();
    let data_start = entry.data_start();
    let method = entry.compression();
    drop(entry);

    if offset > size {
        return Err(ArchiveError::InvalidArchive(format!(
            "offset {offset} is past the end of entry {index} ({size} bytes)"
        )));
    }
    let available = size - offset;
    let take = length.unwrap_or(available).min(available);
    if take == 0 {
        return Ok(0);
    }

    if method == zip::CompressionMethod::Stored {
        // A fresh handle avoids disturbing the archive reader's buffered
        // position; `data_start` is an absolute offset in the archive file.
        let mut data = File::open(path)?;
        data.seek(SeekFrom::Start(data_start.saturating_add(offset)))?;
        copy_exact(&mut data, writer, take)?;
    } else {
        let mut entry = archive.by_index(index).map_err(map_zip_error)?;
        skip_exact(&mut entry, offset)?;
        copy_exact(&mut entry, writer, take)?;
    }
    Ok(take)
}

/// Copies exactly `count` bytes or fails when the source ends early.
fn copy_exact<R: Read + ?Sized, W: Write>(
    reader: &mut R,
    writer: &mut W,
    count: u64,
) -> Result<(), ArchiveError> {
    let mut remaining = count;
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    while remaining > 0 {
        let want = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = reader.read(&mut buffer[..want])?;
        if read == 0 {
            return Err(ArchiveError::InvalidArchive(
                "archive entry ended before its declared size".to_string(),
            ));
        }
        writer.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }
    Ok(())
}

/// Reads and discards exactly `count` bytes or fails when the source ends early.
fn skip_exact<R: Read + ?Sized>(reader: &mut R, count: u64) -> Result<(), ArchiveError> {
    let mut remaining = count;
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    while remaining > 0 {
        let want = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = reader.read(&mut buffer[..want])?;
        if read == 0 {
            return Err(ArchiveError::InvalidArchive(
                "archive entry ended before the requested offset".to_string(),
            ));
        }
        remaining -= read as u64;
    }
    Ok(())
}

fn seven_zip_tar_size(path: &Path) -> Result<u64, ArchiveError> {
    let reader = SevenZReader::open(path, Password::empty()).map_err(map_seven_zip_error)?;
    let entries = collect_seven_zip_tar_entries(reader.archive())?;
    Ok(tar_total_len(entries.iter()))
}

fn stream_seven_zip_as_tar<W: Write>(
    path: &Path,
    writer: &mut W,
) -> Result<TarStreamReport, ArchiveError> {
    let mut reader = SevenZReader::open(path, Password::empty()).map_err(map_seven_zip_error)?;
    let expected = collect_seven_zip_tar_entries(reader.archive())?;
    let expected_bytes = expected
        .iter()
        .fold(0_u64, |total, entry| total.saturating_add(entry.size));

    let mut counter = ByteCounter::new(writer);
    let mut report = TarStreamReport::default();
    {
        let mut builder = tar::Builder::new(&mut counter);
        let mut failure: Option<ArchiveError> = None;
        let outcome = reader.for_each_entries(|entry, data| {
            if failure.is_some() {
                return Ok(false);
            }
            if entry.is_directory() || entry.is_anti_item() {
                return Ok(true);
            }
            let result = (|| -> Result<(), ArchiveError> {
                let relative = sanitize_entry_path(entry.name())?;
                let name = tar_name(&relative);
                let mode = tar_mode(&name, seven_zip_unix_mode(entry.windows_attributes));
                let tar_entry = TarEntry {
                    name,
                    size: entry.size(),
                    mode,
                };
                write_tar_entry(&mut builder, &tar_entry, data)?;
                report.files += 1;
                report.bytes = report.bytes.saturating_add(entry.size());
                Ok(())
            })();
            match result {
                Ok(()) => Ok(true),
                Err(error) => {
                    failure = Some(error);
                    Ok(false)
                }
            }
        });

        if let Some(error) = failure {
            return Err(error);
        }
        outcome.map_err(map_seven_zip_error)?;
        builder.finish()?;
    }
    counter.flush()?;
    report.tar_bytes = counter.count();

    // Guards against a 7z reader that yields a different entry set than its
    // metadata, which would break the advertised Content-Length.
    if report.files != expected.len() as u64 || report.bytes != expected_bytes {
        return Err(ArchiveError::InvalidArchive(
            "7z entry set changed while streaming".to_string(),
        ));
    }
    Ok(report)
}

/// One file entry as it appears in the generated tar stream.
#[derive(Debug, Clone)]
struct TarEntry {
    /// Sanitised relative path with `/` separators.
    name: String,
    /// Uncompressed size in bytes.
    size: u64,
    /// Permission bits (0o644 by default, 0o755 for scripts).
    mode: u32,
}

/// A ZIP entry selected for the tar stream.
#[derive(Debug, Clone)]
struct ZipTarEntry {
    /// Index in the central directory.
    index: usize,
    /// Tar metadata.
    entry: TarEntry,
}

/// Selects and validates the ZIP entries that will be streamed.
fn collect_zip_tar_entries(
    archive: &mut ZipArchive<BufReader<File>>,
) -> Result<Vec<ZipTarEntry>, ArchiveError> {
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        if entry.is_dir() {
            continue;
        }
        if entry.is_symlink() {
            return Err(ArchiveError::UnsafeEntry(format!(
                "link entry: {}",
                entry.name()
            )));
        }
        if entry.encrypted() {
            return Err(ArchiveError::PasswordRequired);
        }
        let relative = sanitize_entry_path(entry.name())?;
        let name = tar_name(&relative);
        let mode = tar_mode(&name, entry.unix_mode());
        entries.push(ZipTarEntry {
            index,
            entry: TarEntry {
                name,
                size: entry.size(),
                mode,
            },
        });
    }
    Ok(entries)
}

/// Selects and validates the 7z entries that will be streamed.
fn collect_seven_zip_tar_entries(archive: &SevenZipArchive) -> Result<Vec<TarEntry>, ArchiveError> {
    if seven_zip_is_encrypted(archive) {
        return Err(ArchiveError::PasswordRequired);
    }
    let mut entries = Vec::new();
    for entry in &archive.files {
        if entry.is_directory() || entry.is_anti_item() {
            continue;
        }
        if is_link_attr(entry.windows_attributes) {
            return Err(ArchiveError::UnsafeEntry(format!(
                "link entry: {}",
                entry.name()
            )));
        }
        let relative = sanitize_entry_path(entry.name())?;
        let name = tar_name(&relative);
        let mode = tar_mode(&name, seven_zip_unix_mode(entry.windows_attributes));
        entries.push(TarEntry {
            name,
            size: entry.size(),
            mode,
        });
    }
    Ok(entries)
}

/// Total tar length for a set of entries: one header + padded data each, plus
/// the two terminating zero blocks.
fn tar_total_len<'a>(entries: impl IntoIterator<Item = &'a TarEntry>) -> u64 {
    entries.into_iter().fold(TAR_END_BLOCKS, |total, entry| {
        total.saturating_add(tar_entry_len(entry))
    })
}

/// Exact tar bytes for one entry, including a GNU long-name entry when the
/// path does not fit in the 100-byte header name field.
fn tar_entry_len(entry: &TarEntry) -> u64 {
    let mut total = TAR_BLOCK.saturating_add(round_up_block(entry.size));
    if entry.name.len() > TAR_NAME_FIELD {
        total = total
            .saturating_add(TAR_BLOCK)
            .saturating_add(round_up_block(entry.name.len() as u64 + 1));
    }
    total
}

/// Rounds a byte length up to the next tar block boundary (0 stays 0).
fn round_up_block(len: u64) -> u64 {
    len.div_ceil(TAR_BLOCK).saturating_mul(TAR_BLOCK)
}

/// Writes one tar header (and GNU long-name header when needed) followed by
/// `data`. The layout matches [`tar_entry_len`] exactly.
fn write_tar_entry<W: Write, R: Read + ?Sized>(
    builder: &mut tar::Builder<W>,
    entry: &TarEntry,
    data: &mut R,
) -> Result<(), ArchiveError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(entry.size);
    header.set_mode(entry.mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);

    if entry.name.len() > TAR_NAME_FIELD {
        let bytes = entry.name.as_bytes();
        let mut long_header = tar::Header::new_gnu();
        long_header.set_entry_type(tar::EntryType::GNULongName);
        long_header.set_size(bytes.len() as u64);
        long_header.set_mode(0o644);
        long_header.set_uid(0);
        long_header.set_gid(0);
        long_header.set_mtime(0);
        long_header.set_path(TAR_LONG_NAME_PATH)?;
        long_header.set_cksum();

        let mut long_name = Vec::with_capacity(bytes.len() + 1);
        long_name.extend_from_slice(bytes);
        long_name.push(0);
        builder.append(&long_header, long_name.as_slice())?;

        header.set_path(truncate_utf8(&entry.name, TAR_NAME_FIELD))?;
    } else {
        header.set_path(entry.name.as_str())?;
    }

    header.set_cksum();
    builder.append(&header, data)?;
    Ok(())
}

/// Converts a sanitised relative path to a `/`-separated tar name.
fn tar_name(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

/// Permission bits for a tar entry: ZIP/7z mode when present, 0o644 otherwise,
/// and 0o755 for shell scripts and AppImages.
fn tar_mode(name: &str, unix_mode: Option<u32>) -> u32 {
    if is_script_name(name) {
        return 0o755;
    }
    match unix_mode {
        Some(mode) if mode & 0o777 != 0 => mode & 0o777,
        _ => 0o644,
    }
}

fn is_script_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("sh") || extension.eq_ignore_ascii_case("appimage")
        })
}

/// Unix permission bits stored in a 7z attribute field, when it looks like a
/// Unix mode rather than a Windows attribute bitmask.
fn seven_zip_unix_mode(attributes: u32) -> Option<u32> {
    if attributes & 0o170_000 == 0 {
        None
    } else {
        Some(attributes & 0o777)
    }
}

/// Truncates `input` to at most `max` bytes on a UTF-8 character boundary.
fn truncate_utf8(input: &str, max: usize) -> &str {
    if input.len() <= max {
        return input;
    }
    let mut end = max;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

/// Write wrapper that counts the bytes that actually reached the sink.
struct ByteCounter<'a, W: Write> {
    inner: &'a mut W,
    bytes: u64,
}

impl<'a, W: Write> ByteCounter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self { inner, bytes: 0 }
    }

    fn count(&self) -> u64 {
        self.bytes
    }
}

impl<W: Write> Write for ByteCounter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.bytes = self.bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        use zip::write::SimpleFileOptions;

        let file = File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).expect("start file");
            writer.write_all(data).expect("write entry");
        }
        writer.finish().expect("finish zip");
    }

    fn write_zip_with_symlink(path: &Path) {
        use zip::write::SimpleFileOptions;

        let file = File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().unix_permissions(0o777);
        writer.start_file("link", options).expect("start symlink");
        writer.write_all(b"../escape").expect("write symlink");
        writer.finish().expect("finish zip");

        // The `zip` writer always records entries as regular files, so patch
        // the central directory to mark the entry as a Unix symlink.
        let mut bytes = fs::read(path).expect("read zip");
        let mut offset = 0;
        while let Some(found) = bytes[offset..]
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
        {
            let position = offset + found;
            bytes[position + 5] = 3; // made by Unix
            bytes[position + 38..position + 42]
                .copy_from_slice(&(0o120_777_u32 << 16).to_le_bytes());
            offset = position + 4;
        }
        fs::write(path, bytes).expect("write patched zip");
    }

    fn write_seven_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = sevenz_rust2::SevenZWriter::create(path).expect("create 7z");
        for (name, data) in entries {
            let entry = SevenZArchiveEntry::new_file(name);
            writer
                .push_archive_entry(entry, Some(Cursor::new(data.to_vec())))
                .expect("push 7z entry");
        }
        writer.finish().expect("finish 7z");
    }

    #[test]
    fn detects_formats_from_magic() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("archive.zip");
        write_zip(&zip_path, &[("a.txt", b"hello")]);
        assert_eq!(
            detect_format(&zip_path).expect("detect zip"),
            ArchiveFormat::Zip
        );

        let rar_path = temp.path().join("archive.rar");
        fs::write(&rar_path, b"Rar!\x1a\x07\x01\x00rest").expect("write rar");
        assert_eq!(
            detect_format(&rar_path).expect("detect rar"),
            ArchiveFormat::Rar
        );

        let seven_path = temp.path().join("archive.7z");
        write_seven_zip(&seven_path, &[("a.txt", b"hello")]);
        assert_eq!(
            detect_format(&seven_path).expect("detect 7z"),
            ArchiveFormat::SevenZip
        );

        let text_path = temp.path().join("notes.txt");
        fs::write(&text_path, b"not an archive").expect("write text");
        assert_eq!(
            detect_format(&text_path).expect("detect text"),
            ArchiveFormat::Unknown
        );

        let empty_path = temp.path().join("empty.bin");
        fs::write(&empty_path, b"").expect("write empty");
        assert_eq!(
            detect_format(&empty_path).expect("detect empty"),
            ArchiveFormat::Unknown
        );

        assert!(detect_format(&temp.path().join("missing.zip")).is_err());
    }

    #[test]
    fn format_from_extension_matches_supported_containers() {
        for name in ["a.zip", "a.ZIP", "GAME (2021).Zip", "/tmp/a.zip"] {
            assert_eq!(
                format_from_extension(Path::new(name)),
                ArchiveFormat::Zip,
                "{name}"
            );
        }
        for name in ["a.7z", "a.7Z"] {
            assert_eq!(
                format_from_extension(Path::new(name)),
                ArchiveFormat::SevenZip,
                "{name}"
            );
        }
        for name in ["a.rar", "a.RAR"] {
            assert_eq!(
                format_from_extension(Path::new(name)),
                ArchiveFormat::Rar,
                "{name}"
            );
        }
        for name in [
            "a.tar",
            "a.gz",
            "a.tar.gz",
            "a.exe",
            "a.zip.part",
            "a",
            "a.txt",
            "",
        ] {
            assert_eq!(
                format_from_extension(Path::new(name)),
                ArchiveFormat::Unknown,
                "{name}"
            );
        }
    }

    #[test]
    fn archive_path_detection() {
        for name in ["a.zip", "a.RAR", "a.7Z", "/tmp/a.zip"] {
            assert!(is_archive_path(Path::new(name)), "{name}");
        }
        for name in ["a.txt", "a.exe", "a.zip.part", "a"] {
            assert!(!is_archive_path(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn sanitize_rejects_escapes() {
        for name in [
            "/etc/passwd",
            "../evil.txt",
            "a/../../evil.txt",
            "a\\..\\..\\evil.txt",
            "C:\\Windows\\evil.txt",
            "C:/Windows/evil.txt",
            "\\\\server\\share\\evil.txt",
            "file.txt:stream",
            "",
            ".",
        ] {
            assert!(
                matches!(sanitize_entry_path(name), Err(ArchiveError::UnsafeEntry(_))),
                "{name}"
            );
        }

        let long = format!("{}.txt", "a".repeat(MAX_ENTRY_PATH_LEN));
        assert!(matches!(
            sanitize_entry_path(&long),
            Err(ArchiveError::UnsafeEntry(_))
        ));
        assert!(matches!(
            sanitize_entry_path("a\0b"),
            Err(ArchiveError::UnsafeEntry(_))
        ));
    }

    #[test]
    fn sanitize_normalises_separators() {
        assert_eq!(
            sanitize_entry_path("dir\\sub/file.txt").expect("safe"),
            PathBuf::from("dir/sub/file.txt")
        );
        assert_eq!(
            sanitize_entry_path("./dir//file.txt").expect("safe"),
            PathBuf::from("dir/file.txt")
        );
        assert_eq!(
            sanitize_entry_path("dir/sub/").expect("safe"),
            PathBuf::from("dir/sub")
        );
    }

    #[test]
    fn verify_zip_counts_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("library.zip");
        write_zip(
            &zip_path,
            &[
                ("readme.txt", b"hello"),
                ("Game/game.exe", b"binary"),
                ("Game/empty.txt", b""),
            ],
        );

        let report = verify_archive(&zip_path, None).expect("verify");
        assert_eq!(report.format, ArchiveFormat::Zip);
        assert_eq!(report.entry_count, 3);
        assert_eq!(report.total_uncompressed, 5 + 6);
        assert!(!report.encrypted);
    }

    #[test]
    fn extract_zip_streams_files_and_progress() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("library.zip");
        write_zip(
            &zip_path,
            &[
                ("readme.txt", b"hello"),
                ("Game/sub/game.exe", b"binary!"),
                ("Game/empty.txt", b""),
                ("Game/unicode-é.txt", "café".as_bytes()),
            ],
        );

        let dest = temp.path().join("out");
        let mut snapshots = Vec::new();
        let report = extract_archive(&zip_path, &dest, &ExtractOptions::default(), &mut |p| {
            snapshots.push(p)
        })
        .expect("extract");

        assert_eq!(report.files, 4);
        assert_eq!(report.bytes, 5 + 7 + 5);
        assert_eq!(
            fs::read(dest.join("Game/sub/game.exe")).expect("read"),
            b"binary!"
        );
        assert_eq!(
            fs::read(dest.join("Game/unicode-é.txt")).expect("read"),
            "café".as_bytes()
        );
        assert!(dest.join("Game/empty.txt").exists());

        assert!(!snapshots.is_empty());
        let last = snapshots.last().expect("snapshot");
        assert_eq!(last.entries_total, 4);
        assert_eq!(last.entries_done, 4);
        assert_eq!(last.bytes_total, 17);
        assert_eq!(last.bytes_done, 17);
    }

    #[test]
    fn extract_zip_rejects_traversal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("evil.zip");
        write_zip(&zip_path, &[("../evil.txt", b"nope")]);

        let dest = temp.path().join("out");
        let error = extract_archive(&zip_path, &dest, &ExtractOptions::default(), &mut |_| {})
            .expect_err("must reject");
        assert!(matches!(error, ArchiveError::UnsafeEntry(_)));
        assert!(!temp.path().join("evil.txt").exists());
    }

    #[test]
    fn extract_zip_rejects_symlinks() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("links.zip");
        write_zip_with_symlink(&zip_path);

        let dest = temp.path().join("out");
        let error = extract_archive(&zip_path, &dest, &ExtractOptions::default(), &mut |_| {})
            .expect_err("must reject");
        assert!(matches!(error, ArchiveError::UnsafeEntry(_)));
    }

    #[test]
    fn extract_zip64_entry() {
        use zip::write::SimpleFileOptions;

        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("zip64.zip");
        let file = File::create(&zip_path).expect("create");
        let mut writer = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().large_file(true);
        writer.start_file("Game/big.txt", options).expect("start");
        writer.write_all(b"zip64 payload").expect("write");
        writer.finish().expect("finish");

        assert_eq!(
            detect_format(&zip_path).expect("detect"),
            ArchiveFormat::Zip
        );
        let report = verify_archive(&zip_path, None).expect("verify");
        assert_eq!(report.entry_count, 1);

        let dest = temp.path().join("out");
        let extracted = extract_archive(&zip_path, &dest, &ExtractOptions::default(), &mut |_| {})
            .expect("extract");
        assert_eq!(extracted.files, 1);
        assert_eq!(
            fs::read(dest.join("Game/big.txt")).expect("read"),
            b"zip64 payload"
        );
    }

    #[test]
    fn seven_zip_roundtrip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive_path = temp.path().join("library.7z");
        write_seven_zip(
            &archive_path,
            &[
                ("readme.txt", b"hello"),
                ("Game/sub/game.sh", b"#!/bin/sh\n"),
                ("Game/empty.txt", b""),
            ],
        );

        let report = verify_archive(&archive_path, None).expect("verify 7z");
        assert_eq!(report.format, ArchiveFormat::SevenZip);
        assert_eq!(report.entry_count, 3);
        assert_eq!(report.total_uncompressed, 5 + 10);

        let dest = temp.path().join("out");
        let mut snapshots = Vec::new();
        let extracted =
            extract_archive(&archive_path, &dest, &ExtractOptions::default(), &mut |p| {
                snapshots.push(p)
            })
            .expect("extract 7z");
        assert_eq!(extracted.files, 3);
        assert_eq!(
            fs::read(dest.join("Game/sub/game.sh")).expect("read"),
            b"#!/bin/sh\n"
        );
        assert!(dest.join("Game/empty.txt").exists());
        let last = snapshots.last().expect("snapshot");
        assert_eq!(last.entries_done, 3);
    }

    #[test]
    fn seven_zip_password_retry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("Game")).expect("mkdir");
        fs::write(source.join("Game/file.txt"), b"secret payload").expect("write");

        let archive_path = temp.path().join("encrypted.7z");
        sevenz_rust2::compress_to_path_encrypted(
            &source,
            &archive_path,
            Password::from("ludex-test"),
        )
        .expect("compress encrypted 7z");

        let no_password = verify_archive(&archive_path, None);
        assert!(
            matches!(no_password, Err(ArchiveError::PasswordRequired)),
            "{no_password:?}"
        );

        let report =
            verify_archive(&archive_path, Some("ludex-test")).expect("verify encrypted 7z");
        assert!(report.encrypted);
        assert_eq!(report.entry_count, 1);

        let dest = temp.path().join("out");
        let error = extract_archive(
            &archive_path,
            &dest,
            &ExtractOptions::default(),
            &mut |_| {},
        )
        .expect_err("password required");
        assert!(error.is_password_error(), "{error:?}");

        let opts = ExtractOptions::with_passwords(vec!["wrong".into(), "ludex-test".into()]);
        let extracted = extract_archive(&archive_path, &dest, &opts, &mut |_| {})
            .expect("extract with second password");
        assert_eq!(extracted.files, 1);
        assert_eq!(
            fs::read(dest.join("Game/file.txt")).expect("read"),
            b"secret payload"
        );
    }

    #[test]
    fn rar_detection_and_graceful_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let rar_path = temp.path().join("fake.rar");
        fs::write(&rar_path, b"Rar!\x1a\x07\x00this is not a real rar").expect("write");

        assert_eq!(
            detect_format(&rar_path).expect("detect"),
            ArchiveFormat::Rar
        );
        let result = verify_archive(&rar_path, None);
        assert!(matches!(result, Err(ArchiveError::InvalidArchive(_))));
    }

    #[test]
    fn unsupported_format_is_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("file.bin");
        fs::write(&path, b"plain text").expect("write");
        let error = extract_archive(&path, temp.path(), &ExtractOptions::default(), &mut |_| {})
            .expect_err("unsupported");
        assert!(matches!(error, ArchiveError::UnsupportedFormat));
    }

    #[test]
    fn overwrite_false_keeps_existing_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("library.zip");
        write_zip(&zip_path, &[("Game/file.txt", b"new")]);

        let dest = temp.path().join("out");
        fs::create_dir_all(dest.join("Game")).expect("mkdir");
        fs::write(dest.join("Game/file.txt"), b"old").expect("write old");

        let opts = ExtractOptions {
            overwrite: false,
            ..ExtractOptions::default()
        };
        let report = extract_archive(&zip_path, &dest, &opts, &mut |_| {}).expect("extract");
        assert_eq!(report.files, 0);
        assert_eq!(fs::read(dest.join("Game/file.txt")).expect("read"), b"old");
    }

    #[test]
    fn progress_fires_per_chunk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("big.zip");
        let payload = vec![7_u8; (PROGRESS_STEP_BYTES as usize) + 1234];
        write_zip(&zip_path, &[("big.bin", &payload)]);

        let dest = temp.path().join("out");
        let mut snapshots = Vec::new();
        let report = extract_archive(&zip_path, &dest, &ExtractOptions::default(), &mut |p| {
            snapshots.push(p)
        })
        .expect("extract");
        assert_eq!(report.bytes, payload.len() as u64);
        assert!(
            snapshots.len() >= 3,
            "expected several progress events, got {}",
            snapshots.len()
        );
        assert!(snapshots
            .iter()
            .any(|snapshot| snapshot.bytes_done >= PROGRESS_STEP_BYTES));
    }

    fn read_tar_entries(buffer: &[u8]) -> Vec<(String, u32, Vec<u8>)> {
        let mut archive = tar::Archive::new(Cursor::new(buffer));
        let mut entries = Vec::new();
        for entry in archive.entries().expect("tar entries") {
            let mut entry = entry.expect("tar entry");
            let path = entry
                .path()
                .expect("tar path")
                .to_string_lossy()
                .to_string();
            let mode = entry.header().mode().expect("tar mode");
            let mut data = Vec::new();
            entry.read_to_end(&mut data).expect("read tar entry");
            entries.push((path, mode, data));
        }
        entries
    }

    #[test]
    fn zip_tar_stream_roundtrip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("library.zip");
        let big = vec![7_u8; 2 * 1024 * 1024];
        write_zip(
            &zip_path,
            &[
                ("Game/readme.txt", b"hello"),
                ("Game/data.bin", &big),
                ("Game/unicode-é.txt", "café".as_bytes()),
                ("Game/run.sh", b"#!/bin/sh\n"),
                ("Game/empty.txt", b""),
            ],
        );

        let size = zip_tar_size(&zip_path).expect("tar size");
        assert!(size > 1024);

        let mut buffer = Vec::new();
        let report = stream_zip_as_tar(&zip_path, &mut buffer).expect("stream tar");
        assert_eq!(report.files, 5);
        assert_eq!(
            report.bytes,
            5 + big.len() as u64 + "café".len() as u64 + 10
        );
        assert_eq!(report.tar_bytes, buffer.len() as u64);
        assert_eq!(report.tar_bytes, size);

        let entries = read_tar_entries(&buffer);
        assert_eq!(entries.len(), 5);
        let find = |name: &str| {
            entries
                .iter()
                .find(|(path, _, _)| path == name)
                .unwrap_or_else(|| panic!("missing tar entry {name}"))
        };
        assert_eq!(find("Game/readme.txt").2, b"hello");
        assert_eq!(find("Game/data.bin").2.len(), big.len());
        assert_eq!(find("Game/data.bin").2, big);
        assert_eq!(find("Game/unicode-é.txt").2, "café".as_bytes());
        assert_eq!(find("Game/run.sh").2, b"#!/bin/sh\n");
        assert_eq!(find("Game/run.sh").1 & 0o777, 0o755);
        assert_eq!(find("Game/readme.txt").1 & 0o777, 0o644);
        assert!(find("Game/empty.txt").2.is_empty());
    }

    #[test]
    fn zip_tar_long_path_is_exact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zip_path = temp.path().join("long.zip");
        let long_name = format!("Game/{}/file.txt", "a".repeat(120));
        write_zip(&zip_path, &[(long_name.as_str(), b"long path payload")]);

        let size = zip_tar_size(&zip_path).expect("tar size");
        assert_eq!(
            size,
            512 + 512 + 512 + 512 + 1024,
            "long-name header + data block + main header + end blocks"
        );

        let mut buffer = Vec::new();
        let report = stream_zip_as_tar(&zip_path, &mut buffer).expect("stream tar");
        assert_eq!(report.tar_bytes, size);
        assert_eq!(report.tar_bytes, buffer.len() as u64);

        let entries = read_tar_entries(&buffer);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, long_name);
        assert_eq!(entries[0].2, b"long path payload");
    }

    #[test]
    fn zip_tar_rejects_non_zip_and_corrupt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let text_path = temp.path().join("notes.txt");
        fs::write(&text_path, b"plain text").expect("write text");
        assert!(matches!(
            zip_tar_size(&text_path),
            Err(ArchiveError::UnsupportedFormat)
        ));
        assert!(matches!(
            stream_zip_as_tar(&text_path, &mut Vec::new()),
            Err(ArchiveError::UnsupportedFormat)
        ));
        assert!(matches!(
            archive_tar_size(&text_path),
            Err(ArchiveError::UnsupportedFormat)
        ));
        assert!(matches!(
            stream_archive_as_tar(&text_path, &mut Vec::new()),
            Err(ArchiveError::UnsupportedFormat)
        ));

        let corrupt_path = temp.path().join("corrupt.zip");
        fs::write(&corrupt_path, b"PK\x03\x04this is not a zip file").expect("write corrupt");
        assert!(matches!(
            zip_tar_size(&corrupt_path),
            Err(ArchiveError::InvalidArchive(_))
        ));
        assert!(matches!(
            stream_zip_as_tar(&corrupt_path, &mut Vec::new()),
            Err(ArchiveError::InvalidArchive(_))
        ));
    }

    #[test]
    fn zip_tar_rejects_traversal_and_links() {
        let temp = tempfile::tempdir().expect("tempdir");

        let traversal_path = temp.path().join("traversal.zip");
        write_zip(&traversal_path, &[("../evil.txt", b"nope")]);
        assert!(matches!(
            zip_tar_size(&traversal_path),
            Err(ArchiveError::UnsafeEntry(_))
        ));
        assert!(matches!(
            stream_zip_as_tar(&traversal_path, &mut Vec::new()),
            Err(ArchiveError::UnsafeEntry(_))
        ));

        let link_path = temp.path().join("link.zip");
        write_zip_with_symlink(&link_path);
        assert!(matches!(
            zip_tar_size(&link_path),
            Err(ArchiveError::UnsafeEntry(_))
        ));
        assert!(matches!(
            stream_zip_as_tar(&link_path, &mut Vec::new()),
            Err(ArchiveError::UnsafeEntry(_))
        ));
    }

    #[test]
    fn seven_zip_tar_stream_roundtrip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive_path = temp.path().join("library.7z");
        write_seven_zip(
            &archive_path,
            &[
                ("Game/alpha.txt", b"alpha"),
                ("Game/beta.sh", b"#!/bin/sh\n"),
                ("Game/empty.txt", b""),
            ],
        );

        let size = archive_tar_size(&archive_path).expect("7z tar size");
        assert!(size > 1024);

        let mut buffer = Vec::new();
        let report = stream_archive_as_tar(&archive_path, &mut buffer).expect("stream 7z tar");
        assert_eq!(report.files, 3);
        assert_eq!(report.bytes, 5 + 10);
        assert_eq!(report.tar_bytes, buffer.len() as u64);
        assert_eq!(report.tar_bytes, size);

        let entries = read_tar_entries(&buffer);
        let find = |name: &str| {
            entries
                .iter()
                .find(|(path, _, _)| path == name)
                .unwrap_or_else(|| panic!("missing tar entry {name}"))
        };
        assert_eq!(find("Game/alpha.txt").2, b"alpha");
        assert_eq!(find("Game/beta.sh").2, b"#!/bin/sh\n");
        assert_eq!(find("Game/beta.sh").1 & 0o777, 0o755);
        assert!(find("Game/empty.txt").2.is_empty());
    }

    #[test]
    fn seven_zip_tar_requires_password() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("Game")).expect("mkdir");
        fs::write(source.join("Game/secret.txt"), b"secret").expect("write");

        let archive_path = temp.path().join("encrypted.7z");
        sevenz_rust2::compress_to_path_encrypted(
            &source,
            &archive_path,
            Password::from("ludex-test"),
        )
        .expect("compress encrypted 7z");

        assert!(matches!(
            archive_tar_size(&archive_path),
            Err(ArchiveError::PasswordRequired)
        ));
        assert!(matches!(
            stream_archive_as_tar(&archive_path, &mut Vec::new()),
            Err(ArchiveError::PasswordRequired)
        ));
    }

    /// Path to a committed test fixture under `crates/ludex-core/tests/fixtures`.
    ///
    /// The RAR fixtures were generated with RAR 6.24 (`rar a -ma4`/`-ma5 -m0`),
    /// the last upstream release that can still create RAR4 archives. The
    /// encrypted fixtures use the password `ludex-test`.
    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn rar4_tar_stream_roundtrip() {
        let rar_path = fixture("rar4.rar");

        let size = archive_tar_size(&rar_path).expect("rar4 tar size");
        assert!(size > 1024);

        let mut buffer = Vec::new();
        let report = stream_archive_as_tar(&rar_path, &mut buffer).expect("stream rar4");
        // Directory entries are skipped; only regular files are streamed.
        assert_eq!(report.files, 4);
        assert_eq!(report.bytes, 5 + 10 + 5);
        assert_eq!(report.tar_bytes, buffer.len() as u64);
        assert_eq!(report.tar_bytes, size);
        assert_eq!(rar_tar_size(&rar_path).expect("rar tar size"), size);

        let entries = read_tar_entries(&buffer);
        assert_eq!(entries.len(), 4);
        let find = |name: &str| {
            entries
                .iter()
                .find(|(path, _, _)| path == name)
                .unwrap_or_else(|| panic!("missing tar entry {name}"))
        };
        assert_eq!(find("Game/readme.txt").2, b"hello");
        assert_eq!(find("Game/sub/game.sh").2, b"#!/bin/sh\n");
        assert_eq!(find("Game/sub/game.sh").1 & 0o777, 0o755);
        assert_eq!(find("Game/readme.txt").1 & 0o777, 0o644);
        assert_eq!(find("Game/unicode-é.txt").2, "café".as_bytes());
        assert!(find("Game/empty.txt").2.is_empty());
    }

    #[test]
    fn rar5_tar_stream_matches_rar4() {
        let rar4 = fixture("rar4.rar");
        let rar5 = fixture("rar5.rar");

        // Both fixtures hold the same four files, so the deterministic tar
        // (mtime 0, canonical order) must be byte-identical.
        assert_eq!(
            archive_tar_size(&rar4).expect("rar4 size"),
            archive_tar_size(&rar5).expect("rar5 size")
        );

        let mut rar4_buffer = Vec::new();
        let rar4_report = stream_rar_as_tar(&rar4, &mut rar4_buffer).expect("stream rar4");
        let mut rar5_buffer = Vec::new();
        let rar5_report = stream_archive_as_tar(&rar5, &mut rar5_buffer).expect("stream rar5");

        assert_eq!(rar4_report, rar5_report);
        assert_eq!(rar4_report.files, 4);
        assert_eq!(rar4_buffer, rar5_buffer);
        assert_eq!(rar5_report.tar_bytes, rar5_buffer.len() as u64);
    }

    #[test]
    fn rar_tar_rejects_encrypted_archives() {
        // File-encrypted archive: entry names are readable but data is not.
        // Header-encrypted archive: even the listing needs a password.
        for name in ["rar5-encrypted.rar", "rar5-encrypted-headers.rar"] {
            let path = fixture(name);
            assert!(
                matches!(archive_tar_size(&path), Err(ArchiveError::PasswordRequired)),
                "{name} size"
            );
            let result = stream_archive_as_tar(&path, &mut Vec::new());
            assert!(
                matches!(result, Err(ArchiveError::PasswordRequired)),
                "{name} stream: {result:?}"
            );
        }
    }

    #[test]
    fn rar_tar_rejects_link_entries() {
        let path = fixture("rar4-symlink.rar");
        assert!(matches!(
            archive_tar_size(&path),
            Err(ArchiveError::UnsafeEntry(_))
        ));
        assert!(matches!(
            stream_archive_as_tar(&path, &mut Vec::new()),
            Err(ArchiveError::UnsafeEntry(_))
        ));
    }

    #[test]
    fn rar_tar_stream_leaves_no_scratch_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut buffer = Vec::new();
        let report = stream_rar_as_tar_in(&fixture("rar4.rar"), &mut buffer, temp.path())
            .expect("stream rar4");

        assert_eq!(report.files, 4);
        let leftovers: Vec<_> = fs::read_dir(temp.path())
            .expect("read scratch base")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        assert!(
            leftovers.is_empty(),
            "scratch files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn tar_size_helpers_match_written_bytes() {
        let entries = vec![
            TarEntry {
                name: "Game/empty.txt".to_string(),
                size: 0,
                mode: 0o644,
            },
            TarEntry {
                name: "Game/sub/data.bin".to_string(),
                size: 700,
                mode: 0o644,
            },
            TarEntry {
                name: format!("Game/{}/long.txt", "x".repeat(150)),
                size: 512,
                mode: 0o755,
            },
        ];
        let expected = tar_total_len(entries.iter());

        let mut builder = tar::Builder::new(Vec::new());
        for entry in &entries {
            let payload = vec![9_u8; entry.size as usize];
            write_tar_entry(&mut builder, entry, &mut payload.as_slice()).expect("write entry");
        }
        builder.finish().expect("finish");
        let bytes = builder.into_inner().expect("into inner");

        assert_eq!(bytes.len() as u64, expected);
        let parsed = read_tar_entries(&bytes);
        assert_eq!(parsed.len(), entries.len());
        for (index, entry) in entries.iter().enumerate() {
            assert_eq!(parsed[index].0, entry.name);
            assert_eq!(parsed[index].1 & 0o777, entry.mode);
            assert_eq!(parsed[index].2.len() as u64, entry.size);
        }
    }

    // -----------------------------------------------------------------------
    // Resumable per-entry manifest (#115)
    // -----------------------------------------------------------------------

    fn write_manifest_zip(path: &Path) {
        use zip::write::SimpleFileOptions;

        let file = File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .add_directory("Game/", SimpleFileOptions::default())
            .expect("directory");
        writer
            .add_directory("Game/data/", SimpleFileOptions::default())
            .expect("nested directory");
        writer
            .start_file(
                "Game/game.exe",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .expect("stored entry");
        writer.write_all(b"MZ ludex").expect("write stored");
        writer
            .start_file(
                "Game/data/big.bin",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .expect("deflated entry");
        let payload: Vec<u8> = (0..200_000).map(|index| (index % 253) as u8).collect();
        writer.write_all(&payload).expect("write deflated");
        writer
            .start_file(
                "Game/run.sh",
                SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(0o700),
            )
            .expect("script entry");
        writer
            .write_all(b"#!/bin/sh\necho hi\n")
            .expect("write script");
        writer.finish().expect("finish zip");
    }

    #[test]
    fn stream_manifest_lists_entries_in_directory_order() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = temp.path().join("manifest.zip");
        write_manifest_zip(&archive);

        let manifest = zip_stream_manifest(&archive).expect("manifest");
        assert_eq!(manifest.format, "zip");
        assert_eq!(manifest.entry_count, manifest.entries.len());
        assert_eq!(manifest.entry_count, 5);
        assert_eq!(
            manifest
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Game",
                "Game/data",
                "Game/game.exe",
                "Game/data/big.bin",
                "Game/run.sh"
            ]
        );
        assert_eq!(
            manifest
                .entries
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(manifest.entries[0].is_dir);
        assert!(manifest.entries[1].is_dir);
        assert_eq!(manifest.entries[2].size, 8);
        assert_eq!(
            manifest.entries[2].crc32,
            crc32fast::hash(b"MZ ludex"),
            "the manifest carries the central-directory CRC-32"
        );
        assert_eq!(manifest.entries[3].size, 200_000);
        assert!(
            manifest.entries[3].compressed_size < manifest.entries[3].size,
            "deflated entries report their compressed size"
        );
        assert_eq!(
            manifest.entries[4].mode, 0o755,
            "scripts keep the executable bit"
        );
        assert_eq!(
            manifest.total_bytes,
            8 + 200_000 + b"#!/bin/sh\necho hi\n".len() as u64
        );

        // The fingerprint is stable and changes with the manifest.
        assert_eq!(manifest.fingerprint(), manifest.fingerprint());
        let mut changed = manifest.clone();
        changed.entries[2].size += 1;
        assert_ne!(manifest.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn entry_ranges_read_stored_and_deflated_data() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = temp.path().join("ranges.zip");
        write_manifest_zip(&archive);
        let manifest = zip_stream_manifest(&archive).expect("manifest");
        let big = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "Game/data/big.bin")
            .expect("big entry");
        let stored = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "Game/game.exe")
            .expect("stored entry");

        let expected: Vec<u8> = (0..200_000).map(|index| (index % 253) as u8).collect();

        // Stored: full read and an arbitrary range.
        let mut full = Vec::new();
        let written =
            write_zip_entry_range(&archive, stored.index, 0, None, &mut full).expect("stored full");
        assert_eq!(written, 8);
        assert_eq!(full, b"MZ ludex");

        let mut ranged = Vec::new();
        let written = write_zip_entry_range(&archive, stored.index, 2, Some(4), &mut ranged)
            .expect("stored range");
        assert_eq!(written, 4);
        assert_eq!(ranged, b" lud");

        // Deflated: the server decompresses and skips to the offset.
        let mut tail = Vec::new();
        let written = write_zip_entry_range(&archive, big.index, 199_990, None, &mut tail)
            .expect("deflated tail");
        assert_eq!(written, 10);
        assert_eq!(tail, expected[199_990..]);

        let mut middle = Vec::new();
        let written = write_zip_entry_range(&archive, big.index, 1000, Some(16), &mut middle)
            .expect("middle");
        assert_eq!(written, 16);
        assert_eq!(middle, expected[1000..1016]);

        // A range that runs past the end is clamped to the entry.
        let mut clamped = Vec::new();
        let written = write_zip_entry_range(&archive, big.index, 199_990, Some(100), &mut clamped)
            .expect("clamped");
        assert_eq!(written, 10);

        // Offset exactly at the end and past it.
        let mut empty = Vec::new();
        assert_eq!(
            write_zip_entry_range(&archive, big.index, 200_000, None, &mut empty).expect("end"),
            0
        );
        assert!(matches!(
            write_zip_entry_range(&archive, big.index, 200_001, None, &mut Vec::new()),
            Err(ArchiveError::InvalidArchive(_))
        ));
    }

    #[test]
    fn entry_info_matches_the_manifest_and_rejects_bad_indexes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = temp.path().join("info.zip");
        write_manifest_zip(&archive);
        let manifest = zip_stream_manifest(&archive).expect("manifest");
        for entry in &manifest.entries {
            assert_eq!(&zip_entry_info(&archive, entry.index).expect("info"), entry);
        }
        assert!(matches!(
            zip_entry_info(&archive, manifest.entry_count + 1),
            Err(ArchiveError::EntryNotFound)
        ));
        let directory = zip_entry_info(&archive, 0).expect("directory");
        assert!(directory.is_dir);
        assert_eq!(directory.size, 0);
    }

    #[test]
    fn manifest_rejects_traversal_links_and_non_zip_archives() {
        let temp = tempfile::tempdir().expect("tempdir");
        let traversal = temp.path().join("traversal.zip");
        write_zip(&traversal, &[("../escape.txt", b"nope")]);
        assert!(matches!(
            zip_stream_manifest(&traversal),
            Err(ArchiveError::UnsafeEntry(_))
        ));

        let link = temp.path().join("link.zip");
        write_zip_with_symlink(&link);
        assert!(matches!(
            zip_stream_manifest(&link),
            Err(ArchiveError::UnsafeEntry(_))
        ));

        let text = temp.path().join("not-an-archive.zip");
        fs::write(&text, b"plain text").expect("write text");
        assert!(matches!(
            zip_stream_manifest(&text),
            Err(ArchiveError::UnsupportedFormat)
        ));
    }
}
