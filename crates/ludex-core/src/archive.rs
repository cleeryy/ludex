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
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use sevenz_rust2::{Archive as SevenZipArchive, Password, SevenZArchiveEntry, SevenZReader};
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
        ZipError::FileNotFound => ArchiveError::InvalidArchive("entry not found".to_string()),
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
}
