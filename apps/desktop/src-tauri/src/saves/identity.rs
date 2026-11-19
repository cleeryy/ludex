//! Cross-platform save identity (#294).
//!
//! A save is identified by the game id, the save-root kind and the path below
//! that root — never by an absolute host path. The same identity is resolved on
//! the current machine: a Windows known folder (`Documents`, `Saved Games`,
//! `AppData/Roaming`, `AppData/Local`, …) maps to the matching folder inside a
//! Wine/Proton prefix (`drive_c/users/<user>/Documents`, …) and back.
//!
//! The identity contains no drive letter, user name or prefix path, so it is
//! safe to keep in a status payload. Resolution is case-insensitive under a
//! Windows/Proton root (NTFS semantics) and reads the casing that exists on
//! disk, which is what makes a case-only difference the same save.
//!
//! Nothing here opens a save for writing. A pair is only trusted after its
//! file names, extension signature and size look like the same tree; several
//! equally plausible candidates are reported as ambiguous, never picked.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::saves::detect::Platform;

/// Prefix of the serialized identity. Bump the version when the meaning of the
/// root kinds changes, so an old stored mapping is rejected instead of
/// misinterpreted.
pub const IDENTITY_SCHEME: &str = "ludex-save:1";

const MAX_RELATIVE_CHARS: usize = 512;
const MAX_SIGNATURE_FILES: u64 = 512;
const MAX_SIGNATURE_DEPTH: usize = 8;
/// Text files above this are copied raw: they are not edited by hand and a
/// large read per save would cost more than the normalisation is worth.
pub const MAX_TEXT_NORMALIZE_BYTES: u64 = 16 * 1024 * 1024;
/// Case-insensitive roots read this many entries when looking for the real
/// casing of one segment. A save root with more entries is not a save root.
const MAX_CASE_ENTRIES: usize = 4_000;

/// Save roots that can be translated. `Home`, the XDG folders and
/// `ApplicationSupport` have no Windows equivalent, so an identity rooted there
/// is reported as one-platform instead of missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RootKind {
    Documents,
    SavedGames,
    RoamingAppData,
    LocalAppData,
    LocalAppDataLow,
    ProgramData,
    Public,
    Home,
    XdgData,
    XdgConfig,
    XdgState,
    ApplicationSupport,
}

impl RootKind {
    pub const ALL: [RootKind; 12] = [
        RootKind::Documents,
        RootKind::SavedGames,
        RootKind::RoamingAppData,
        RootKind::LocalAppData,
        RootKind::LocalAppDataLow,
        RootKind::ProgramData,
        RootKind::Public,
        RootKind::Home,
        RootKind::XdgData,
        RootKind::XdgConfig,
        RootKind::XdgState,
        RootKind::ApplicationSupport,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            RootKind::Documents => "documents",
            RootKind::SavedGames => "saved-games",
            RootKind::RoamingAppData => "roaming-app-data",
            RootKind::LocalAppData => "local-app-data",
            RootKind::LocalAppDataLow => "local-app-data-low",
            RootKind::ProgramData => "program-data",
            RootKind::Public => "public",
            RootKind::Home => "home",
            RootKind::XdgData => "xdg-data",
            RootKind::XdgConfig => "xdg-config",
            RootKind::XdgState => "xdg-state",
            RootKind::ApplicationSupport => "application-support",
        }
    }

    pub fn parse_slug(value: &str) -> Option<Self> {
        RootKind::ALL
            .iter()
            .copied()
            .find(|kind| kind.slug() == value)
    }

    /// A Windows-only known folder: it only exists with a Wine/Proton prefix on
    /// Linux, never as a native folder.
    pub fn windows_only(self) -> bool {
        matches!(
            self,
            RootKind::SavedGames
                | RootKind::RoamingAppData
                | RootKind::LocalAppData
                | RootKind::LocalAppDataLow
                | RootKind::ProgramData
                | RootKind::Public
        )
    }

    /// A root that can be expressed on both sides. `Documents` is the bridge:
    /// native on Windows and macOS, an XDG folder on Linux, and
    /// `drive_c/users/<user>/Documents` inside a prefix.
    pub fn windows_mappable(self) -> bool {
        self.windows_only() || self == RootKind::Documents
    }

    /// Human label used in status messages, never in the identity itself.
    pub fn label(self) -> &'static str {
        match self {
            RootKind::Documents => "Documents",
            RootKind::SavedGames => "Saved Games",
            RootKind::RoamingAppData => "AppData/Roaming",
            RootKind::LocalAppData => "AppData/Local",
            RootKind::LocalAppDataLow => "AppData/LocalLow",
            RootKind::ProgramData => "ProgramData",
            RootKind::Public => "Public",
            RootKind::Home => "Home",
            RootKind::XdgData => "XDG data",
            RootKind::XdgConfig => "XDG config",
            RootKind::XdgState => "XDG state",
            RootKind::ApplicationSupport => "Application Support",
        }
    }
}

/// Where a path physically lives. `Proton` is a Wine/Proton `drive_c` tree on a
/// POSIX host; it is not the same layout as a native Windows drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Windows,
    Proton,
    Linux,
    Macos,
}

impl Layout {
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Windows => "windows",
            Layout::Proton => "proton",
            Layout::Linux => "linux",
            Layout::Macos => "macos",
        }
    }

    pub fn current() -> Self {
        if cfg!(windows) {
            Layout::Windows
        } else if cfg!(target_os = "macos") {
            Layout::Macos
        } else {
            Layout::Linux
        }
    }

    fn case_insensitive(self) -> bool {
        matches!(self, Layout::Windows | Layout::Proton)
    }
}

/// A filesystem path resolved back to a root kind and a relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedPath {
    pub root: RootKind,
    pub relative: String,
    pub layout: Layout,
}

/// Logical save identity: game + root kind + relative path. No absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveIdentity {
    pub game_id: i64,
    pub root: RootKind,
    pub relative: String,
}

impl SaveIdentity {
    pub fn new(game_id: i64, root: RootKind, relative: &str) -> Result<Self, String> {
        let relative = sanitize_relative(relative)
            .ok_or_else(|| "A save identity needs a relative path below its root.".to_string())?;
        Ok(Self {
            game_id,
            root,
            relative,
        })
    }

    /// `ludex-save:1/<game>/<root>/<relative>`; no host path is included.
    pub fn serialized(&self) -> String {
        format!(
            "{IDENTITY_SCHEME}/{}/{}/{}",
            self.game_id,
            self.root.slug(),
            self.relative
        )
    }

    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.trim().strip_prefix(IDENTITY_SCHEME)?;
        let rest = rest.strip_prefix('/')?;
        let mut parts = rest.splitn(3, '/');
        let game_id = parts.next()?.parse::<i64>().ok()?;
        let root = RootKind::parse_slug(parts.next()?)?;
        let relative = parts.next()?;
        Self::new(game_id, root, relative).ok()
    }

    /// Comparison key: case-folded, because a case-only difference is the same
    /// save on Windows and in a Proton prefix.
    pub fn key(&self) -> String {
        self.serialized().to_lowercase()
    }

    pub fn relative(&self) -> &str {
        &self.relative
    }

    pub fn root(&self) -> RootKind {
        self.root
    }
}

/// One resolved root on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SaveRoot {
    kind: RootKind,
    path: PathBuf,
    layout: Layout,
}

/// The roots available on this machine, plus the Wine/Proton prefixes that can
/// supply a Windows-shaped root.
#[derive(Debug, Clone, Default)]
pub struct SaveRoots {
    roots: Vec<SaveRoot>,
    prefixes: Vec<PathBuf>,
}

impl SaveRoots {
    pub fn add(&mut self, kind: RootKind, path: impl Into<PathBuf>, layout: Layout) {
        let path = path.into();
        if self
            .roots
            .iter()
            .any(|root| root.kind == kind && root.path == path)
        {
            return;
        }
        self.roots.push(SaveRoot { kind, path, layout });
    }

    pub fn add_prefix(&mut self, prefix: &Path) {
        let Some(prefix) = normalize_prefix(prefix) else {
            return;
        };
        if !self.prefixes.contains(&prefix) {
            self.prefixes.push(prefix);
        }
    }

    /// Register the prefix a save path lives in, so a later translation keeps
    /// considering it even when the global setting is empty.
    pub fn add_prefix_from_path(&mut self, path: &Path) {
        if let Some(prefix) = prefix_of(path) {
            self.add_prefix(&prefix);
        }
    }

    /// Classify an absolute host path against the known roots, then fall back
    /// to the structural Windows/Proton layout for paths whose root was not
    /// registered (a mounted drive, a prefix not in the settings).
    pub fn classify(&self, path: &Path) -> Option<ClassifiedPath> {
        if let Some(found) = self.classify_roots(path) {
            return Some(found);
        }
        classify_windows_text(&path.to_string_lossy())
    }

    fn classify_roots(&self, path: &Path) -> Option<ClassifiedPath> {
        let tokens = path_tokens(path)?;
        let mut best: Option<(usize, ClassifiedPath)> = None;
        for root in &self.roots {
            let Some(root_tokens) = path_tokens(&root.path) else {
                continue;
            };
            if root_tokens.len() >= tokens.len() {
                continue;
            }
            let case_insensitive = root.layout.case_insensitive();
            let matches = root_tokens.iter().zip(tokens.iter()).all(|(a, b)| {
                if case_insensitive {
                    a.eq_ignore_ascii_case(b)
                } else {
                    a == b
                }
            });
            if !matches {
                continue;
            }
            let relative = tokens[root_tokens.len()..].join("/");
            if relative.is_empty() {
                continue;
            }
            let better = best
                .as_ref()
                .map(|(depth, _)| root_tokens.len() > *depth)
                .unwrap_or(true);
            if better {
                best = Some((
                    root_tokens.len(),
                    ClassifiedPath {
                        root: root.kind,
                        relative,
                        layout: root.layout,
                    },
                ));
            }
        }
        best.map(|(_, classified)| classified)
    }

    /// Every path this identity could name on this machine. The paths are not
    /// checked for existence; callers filter. Order is deterministic.
    pub fn candidates(&self, identity: &SaveIdentity) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();
        for root in self
            .roots
            .iter()
            .filter(|root| root.kind == identity.root())
        {
            push_unique(
                &mut paths,
                join_case_insensitive(&root.path, identity.relative()),
            );
        }
        if identity.root().windows_mappable() {
            for prefix in &self.prefixes {
                for base in windows_bases(prefix, identity.root()) {
                    push_unique(
                        &mut paths,
                        join_case_insensitive(&base, identity.relative()),
                    );
                }
            }
        }
        paths
    }

    /// True when this platform can express the identity at all. An XDG root on
    /// Windows is not "missing": it only exists on Linux/macOS.
    pub fn resolvable(&self, identity: &SaveIdentity) -> bool {
        !self.candidates(identity).is_empty()
    }

    /// One platform-aware resolver for the current machine. Never invents a
    /// root: a folder that does not resolve is simply absent.
    pub fn current(wine_prefix: &str, settings_steam_root: &str) -> Self {
        let platform = Platform::current();
        let folders = crate::saves::registry::system_known_folders(platform);
        let mut roots = Self::default();
        for (id, path) in folders.iter() {
            if let Some(kind) = kind_for_folder_id(id) {
                roots.add(kind, path.clone(), layout_for_folder(kind, platform));
            }
        }
        roots.add_prefix(Path::new(wine_prefix));
        if let Some(prefix) = std::env::var_os("WINEPREFIX") {
            roots.add_prefix(Path::new(&prefix));
        }
        if let Some(home) = folders.get("home") {
            roots.add_prefix(&home.join(".wine"));
            let steam = settings_steam_root.trim();
            let steam_root = if steam.is_empty() {
                home.join(".local/share/Steam")
            } else {
                PathBuf::from(steam)
            };
            roots.add_steam_prefixes(&steam_root);
        }
        roots
    }

    /// `compatdata/<appid>/pfx` folders under a Steam library. A missing
    /// `steamapps` folder is skipped without an error.
    fn add_steam_prefixes(&mut self, steam_root: &Path) {
        let compatdata = steam_root.join("steamapps/compatdata");
        let Ok(entries) = fs::read_dir(&compatdata) else {
            return;
        };
        let mut prefixes: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path().join("pfx"))
            .filter(|prefix| prefix.is_dir())
            .collect();
        prefixes.sort();
        for prefix in prefixes {
            self.add_prefix(&prefix);
        }
    }
}

/// Map a registry known-folder id to an identity root kind.
pub fn kind_for_folder_id(id: &str) -> Option<RootKind> {
    match id {
        "documents" => Some(RootKind::Documents),
        "saved_games" => Some(RootKind::SavedGames),
        "roaming_app_data" => Some(RootKind::RoamingAppData),
        "local_app_data" => Some(RootKind::LocalAppData),
        "local_app_data_low" => Some(RootKind::LocalAppDataLow),
        "program_data" => Some(RootKind::ProgramData),
        "public" => Some(RootKind::Public),
        "home" => Some(RootKind::Home),
        "xdg_data" => Some(RootKind::XdgData),
        "xdg_config" => Some(RootKind::XdgConfig),
        "xdg_state" => Some(RootKind::XdgState),
        "application_support" => Some(RootKind::ApplicationSupport),
        _ => None,
    }
}

fn layout_for_folder(kind: RootKind, platform: Platform) -> Layout {
    match kind {
        _ if kind.windows_only() => {
            if platform == Platform::Windows {
                Layout::Windows
            } else {
                Layout::Proton
            }
        }
        RootKind::Documents => match platform {
            Platform::Windows => Layout::Windows,
            Platform::Macos => Layout::Macos,
            Platform::Linux => Layout::Linux,
        },
        RootKind::Home => match platform {
            Platform::Windows => Layout::Windows,
            Platform::Macos => Layout::Macos,
            Platform::Linux => Layout::Linux,
        },
        RootKind::ApplicationSupport => Layout::Macos,
        _ => Layout::Linux,
    }
}

/* ----------------------------------------------------- windows structure */

fn is_drive(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

fn classify_windows_text(text: &str) -> Option<ClassifiedPath> {
    let normalized = text.replace('\\', "/");
    let raw: Vec<&str> = normalized
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    if raw.is_empty() || raw.contains(&"..") {
        return None;
    }
    let lower: Vec<String> = raw
        .iter()
        .map(|segment| segment.to_ascii_lowercase())
        .collect();
    let (layout, start) = if let Some(index) = lower.iter().position(|segment| segment == "drive_c")
    {
        (Layout::Proton, index + 1)
    } else if raw.first().copied().is_some_and(is_drive) {
        (Layout::Windows, 1)
    } else {
        return None;
    };
    let rest = raw.get(start..)?;
    if rest
        .first()
        .is_some_and(|first| first.eq_ignore_ascii_case("programdata"))
    {
        return classified(RootKind::ProgramData, &rest[1..], layout);
    }
    if !rest
        .first()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("users"))
    {
        return None;
    }
    let user = rest.get(1)?;
    if user.eq_ignore_ascii_case("public") {
        return classified(RootKind::Public, &rest[2..], layout);
    }
    let after = rest.get(2..)?;
    let first = after.first()?;
    if first.eq_ignore_ascii_case("documents") {
        return classified(RootKind::Documents, &after[1..], layout);
    }
    if first.to_ascii_lowercase().starts_with("onedrive") {
        // OneDrive Known Folder Move: `<user>/OneDrive[ - org]/Documents`.
        let documents = after
            .iter()
            .position(|segment| segment.eq_ignore_ascii_case("documents"))?;
        return classified(RootKind::Documents, &after[documents + 1..], layout);
    }
    if first.eq_ignore_ascii_case("saved games") {
        return classified(RootKind::SavedGames, &after[1..], layout);
    }
    if first.eq_ignore_ascii_case("appdata") {
        let sub = after.get(1)?;
        let (kind, rest) = if sub.eq_ignore_ascii_case("roaming") {
            (RootKind::RoamingAppData, &after[2..])
        } else if sub.eq_ignore_ascii_case("local") {
            (RootKind::LocalAppData, &after[2..])
        } else if sub.eq_ignore_ascii_case("locallow") {
            (RootKind::LocalAppDataLow, &after[2..])
        } else {
            return None;
        };
        return classified(kind, rest, layout);
    }
    None
}

fn classified(root: RootKind, rest: &[&str], layout: Layout) -> Option<ClassifiedPath> {
    let relative = sanitize_relative(&rest.join("/"))?;
    Some(ClassifiedPath {
        root,
        relative,
        layout,
    })
}

/// Infer the layout of a path whose root was not registered. Used to report a
/// one-platform root instead of a misleading "missing".
pub fn infer_layout(path: &Path) -> Option<Layout> {
    if let Some(found) = classify_windows_text(&path.to_string_lossy()) {
        return Some(found.layout);
    }
    let text = path.to_string_lossy();
    if text.starts_with('/') {
        if cfg!(windows) {
            return Some(Layout::Linux);
        }
        return Some(Layout::current());
    }
    if is_drive(text.split(['/', '\\']).next().unwrap_or("")) {
        return Some(Layout::Windows);
    }
    None
}

/* --------------------------------------------------------------- helpers */

fn sanitize_relative(value: &str) -> Option<String> {
    let value = value.trim().replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for part in value.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.chars().any(char::is_control) {
            return None;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("/");
    if joined.len() > MAX_RELATIVE_CHARS {
        return None;
    }
    Some(joined)
}

/// Path components as strings; a `..` makes the path unclassifiable.
fn path_tokens(path: &Path) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                tokens.push(prefix.as_os_str().to_string_lossy().to_string());
            }
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => return None,
            Component::Normal(value) => tokens.push(value.to_string_lossy().to_string()),
        }
    }
    Some(tokens)
}

fn normalize_prefix(prefix: &Path) -> Option<PathBuf> {
    let text = prefix.to_string_lossy().trim().to_string();
    if text.is_empty() {
        return None;
    }
    let mut prefix = PathBuf::from(text);
    if prefix
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("drive_c"))
    {
        prefix.pop();
    }
    Some(prefix)
}

fn prefix_of(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("drive_c"))
        })
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

fn prefix_drive_c(prefix: &Path) -> PathBuf {
    if prefix.ends_with("drive_c") {
        prefix.to_path_buf()
    } else {
        prefix.join("drive_c")
    }
}

fn windows_users(drive_c: &Path) -> Vec<String> {
    let mut users: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(drive_c.join("users")) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "public" | "default" | "default user" | "all users"
            ) {
                continue;
            }
            users.push(name);
        }
    }
    users.sort();
    users.sort_by_key(|name| (!name.eq_ignore_ascii_case("steamuser"), name.clone()));
    if users.is_empty() {
        users.push("steamuser".to_string());
    }
    users
}

/// Windows-shaped folders inside one prefix for a root kind.
fn windows_bases(prefix: &Path, kind: RootKind) -> Vec<PathBuf> {
    let drive_c = prefix_drive_c(prefix);
    if kind == RootKind::ProgramData {
        return vec![drive_c.join("ProgramData")];
    }
    if kind == RootKind::Public {
        return vec![drive_c.join("users/Public")];
    }
    let folder = match kind {
        RootKind::Documents => "Documents",
        RootKind::SavedGames => "Saved Games",
        RootKind::RoamingAppData => "AppData/Roaming",
        RootKind::LocalAppData => "AppData/Local",
        RootKind::LocalAppDataLow => "AppData/LocalLow",
        _ => return Vec::new(),
    };
    windows_users(&drive_c)
        .into_iter()
        .map(|user| drive_c.join("users").join(user).join(folder))
        .collect()
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

/// Join the relative path, preferring the casing that exists on disk. A
/// segment that does not exist keeps the identity's casing, which is what the
/// game will create on this platform.
fn join_case_insensitive(base: &Path, relative: &str) -> PathBuf {
    let mut current = base.to_path_buf();
    for segment in relative.split('/') {
        current = existing_child(&current, segment);
    }
    current
}

fn existing_child(parent: &Path, name: &str) -> PathBuf {
    let direct = parent.join(name);
    if direct.exists() {
        return direct;
    }
    let Ok(entries) = fs::read_dir(parent) else {
        return direct;
    };
    let mut matches: Vec<PathBuf> = entries
        .flatten()
        .take(MAX_CASE_ENTRIES)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        })
        .map(|entry| entry.path())
        .collect();
    matches.sort();
    matches.into_iter().next().unwrap_or(direct)
}

/* ------------------------------------------------------------ validation */

/// Bounded fingerprint of a save tree: file names, extensions and total size.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeSignature {
    pub files: u64,
    pub bytes: u64,
    pub names: BTreeSet<String>,
    pub extensions: BTreeSet<String>,
}

impl TreeSignature {
    pub fn is_empty(&self) -> bool {
        self.files == 0
    }
}

pub fn tree_signature(path: &Path) -> TreeSignature {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return TreeSignature::default();
    };
    if metadata.file_type().is_symlink() {
        return TreeSignature::default();
    }
    if metadata.is_file() {
        let mut signature = TreeSignature::default();
        add_signature_file(&mut signature, path, Path::new(""));
        return signature;
    }
    if !metadata.is_dir() {
        return TreeSignature::default();
    }
    let mut signature = TreeSignature::default();
    walk_signature(&mut signature, path, path, 0);
    signature
}

fn walk_signature(signature: &mut TreeSignature, root: &Path, directory: &Path, depth: usize) {
    if depth > MAX_SIGNATURE_DEPTH || signature.files >= MAX_SIGNATURE_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if signature.files >= MAX_SIGNATURE_FILES {
            return;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            walk_signature(signature, root, &path, depth + 1);
        } else if metadata.is_file() {
            add_signature_file(signature, &path, root);
        }
    }
}

fn add_signature_file(signature: &mut TreeSignature, path: &Path, root: &Path) {
    let name = path.strip_prefix(root).unwrap_or(path);
    let name = name.to_string_lossy().replace('\\', "/").to_lowercase();
    if name.is_empty() {
        return;
    }
    signature.files += 1;
    signature.bytes = signature
        .bytes
        .saturating_add(fs::symlink_metadata(path).map(|m| m.len()).unwrap_or(0));
    signature.names.insert(name);
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        signature.extensions.insert(extension.to_lowercase());
    }
}

/// Result of comparing two trees that should hold the same save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairVerdict {
    Trusted,
    EmptyTarget,
    NotSameIdentity,
    SignatureDiffers,
    FileNamesDiffer {
        missing: Vec<String>,
        extra: Vec<String>,
    },
    ImplausibleSize {
        source_bytes: u64,
        target_bytes: u64,
    },
}

impl PairVerdict {
    pub fn trusted(&self) -> bool {
        matches!(self, PairVerdict::Trusted)
    }

    /// One sentence for the status/log, never a dump of the whole tree.
    pub fn describe(&self) -> String {
        match self {
            PairVerdict::Trusted => "same save".to_string(),
            PairVerdict::EmptyTarget => "the mapped folder contains no files".to_string(),
            PairVerdict::NotSameIdentity => {
                "the mapped folder is a different save root".to_string()
            }
            PairVerdict::SignatureDiffers => "the save file types do not match".to_string(),
            PairVerdict::FileNamesDiffer { missing, extra } => {
                format!(
                    "file names differ (missing {}, extra {})",
                    short_list(missing),
                    short_list(extra)
                )
            }
            PairVerdict::ImplausibleSize {
                source_bytes,
                target_bytes,
            } => format!("sizes differ too much ({source_bytes} vs {target_bytes} bytes)"),
        }
    }
}

fn short_list(values: &[String]) -> String {
    if values.is_empty() {
        return "none".to_string();
    }
    let mut listed = values
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > 3 {
        listed.push_str(&format!(" (+{})", values.len() - 3));
    }
    listed
}

fn sizes_plausible(source: u64, target: u64) -> bool {
    let source = u128::from(source);
    let target = u128::from(target);
    let slack = u128::from(4096u64);
    target + slack >= source / 2 && target <= source * 2 + slack
}

/// Validate a translated pair before trusting it. Names must match
/// case-insensitively, the extension signature must overlap, and the byte total
/// must stay within 0.5×–2× (a line-ending difference is a few percent).
pub fn pair_verdict(
    roots: &SaveRoots,
    identity: &SaveIdentity,
    source: &Path,
    target: &Path,
) -> PairVerdict {
    let same_identity = roots
        .classify(target)
        .and_then(|classified| {
            SaveIdentity::new(identity.game_id, classified.root, &classified.relative).ok()
        })
        .map(|other| other.key() == identity.key())
        .unwrap_or(true);
    if !same_identity {
        return PairVerdict::NotSameIdentity;
    }
    let source_signature = tree_signature(source);
    let target_signature = tree_signature(target);
    if target_signature.is_empty() {
        return PairVerdict::EmptyTarget;
    }
    if source_signature.is_empty() {
        // The source is not readable; identity alone is the only evidence.
        return PairVerdict::Trusted;
    }
    if !source_signature.extensions.is_empty()
        && !target_signature.extensions.is_empty()
        && source_signature
            .extensions
            .is_disjoint(&target_signature.extensions)
    {
        return PairVerdict::SignatureDiffers;
    }
    if source_signature.names != target_signature.names {
        let missing = source_signature
            .names
            .difference(&target_signature.names)
            .cloned()
            .collect();
        let extra = target_signature
            .names
            .difference(&source_signature.names)
            .cloned()
            .collect();
        return PairVerdict::FileNamesDiffer { missing, extra };
    }
    if !sizes_plausible(source_signature.bytes, target_signature.bytes) {
        return PairVerdict::ImplausibleSize {
            source_bytes: source_signature.bytes,
            target_bytes: target_signature.bytes,
        };
    }
    PairVerdict::Trusted
}

/// How an identity can be resolved on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingPlan {
    /// Exactly one existing candidate survived validation.
    Resolved { path: PathBuf },
    /// Several candidates are equally plausible. The caller asks the user once
    /// and never picks for them.
    Ambiguous { candidates: Vec<PathBuf> },
    /// A candidate existed but failed validation against the source tree.
    Rejected { candidate: PathBuf, reason: String },
    /// No candidate exists on this platform.
    Unresolved,
}

/// Resolve an identity for the current machine. `source` is the local path the
/// identity was derived from, when it still exists: it is the reference the
/// candidates are validated against.
pub fn plan_mapping(
    identity: &SaveIdentity,
    roots: &SaveRoots,
    source: Option<&Path>,
) -> MappingPlan {
    let existing: Vec<PathBuf> = roots
        .candidates(identity)
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    if existing.is_empty() {
        return MappingPlan::Unresolved;
    }
    // Without a readable source there is nothing to validate against. One
    // candidate is the identity's own path; several are ambiguous.
    let source = source.filter(|path| tree_signature(path).files > 0);
    let mut kept = Vec::new();
    let mut rejected = None;
    for candidate in existing {
        match source {
            Some(source) => {
                let verdict = pair_verdict(roots, identity, source, &candidate);
                if verdict.trusted() {
                    kept.push(candidate);
                } else if rejected.is_none() {
                    rejected = Some((candidate, verdict.describe()));
                }
            }
            None => kept.push(candidate),
        }
    }
    match kept.len() {
        0 => match rejected {
            Some((candidate, reason)) => MappingPlan::Rejected { candidate, reason },
            None => MappingPlan::Unresolved,
        },
        1 => MappingPlan::Resolved {
            path: kept.remove(0),
        },
        _ => MappingPlan::Ambiguous { candidates: kept },
    }
}

/* --------------------------------------------------------- normalisation */

/// Extensions normalised to LF before an archive is hashed. Only formats that
/// games read as text: a save with a NUL byte or invalid UTF-8 is copied
/// byte-for-byte.
pub const TEXT_EXTENSIONS: &[&str] = &[
    "cfg",
    "conf",
    "config",
    "csv",
    "ini",
    "json",
    "lua",
    "properties",
    "settings",
    "toml",
    "txt",
    "xml",
    "yaml",
    "yml",
];

pub fn is_text_save_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            TEXT_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// `Some` only when the bytes are a UTF-8 text file that actually contains a
/// CRLF pair. Otherwise the caller copies the file untouched.
pub fn normalize_text_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.windows(2).any(|window| window == b"\r\n") || bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.replace("\r\n", "\n").into_bytes())
}

/* -------------------------------------------------------------- mappings */

/// A user decision for one ambiguous identity, persisted in `settings.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SaveIdentityMapping {
    pub game_id: i64,
    pub identity: String,
    pub path: String,
}

/// Trim, drop empties and keep one mapping per game + identity.
pub fn normalize_mappings(mappings: &mut Vec<SaveIdentityMapping>) {
    mappings.retain_mut(|mapping| {
        mapping.identity = mapping.identity.trim().to_string();
        mapping.path = mapping.path.trim().to_string();
        !mapping.identity.is_empty() && !mapping.path.is_empty()
    });
    let mut seen = std::collections::HashSet::new();
    mappings.retain(|mapping| seen.insert((mapping.game_id, mapping.identity.to_lowercase())));
    if mappings.len() > 512 {
        mappings.truncate(512);
    }
}

/* ------------------------------------------------------------------ tests */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::short_id;

    struct Fixture {
        root: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "ludex-identity-{label}-{}",
            short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&root).unwrap();
        Fixture { root }
    }

    fn windows_roots(documents: &Path) -> SaveRoots {
        let mut roots = SaveRoots::default();
        roots.add(RootKind::Documents, documents, Layout::Windows);
        roots
    }

    #[test]
    fn identity_roundtrips_and_has_no_absolute_path() {
        let identity =
            SaveIdentity::new(42, RootKind::Documents, "My Games/Hades/profile.sav").unwrap();
        let serialized = identity.serialized();
        assert_eq!(
            serialized,
            "ludex-save:1/42/documents/My Games/Hades/profile.sav"
        );
        assert!(!serialized.contains("C:"));
        assert!(!serialized.contains("Users"));
        assert!(!serialized.contains("steamuser"));
        assert_eq!(SaveIdentity::parse(&serialized), Some(identity));
    }

    #[test]
    fn identity_rejects_navigation_and_empty_relative_paths() {
        assert!(SaveIdentity::new(1, RootKind::Documents, "../escape").is_err());
        assert!(SaveIdentity::new(1, RootKind::Documents, "").is_err());
        assert!(SaveIdentity::new(1, RootKind::Documents, "game/\u{7}bad").is_err());
    }

    #[test]
    fn windows_documents_translate_to_the_proton_prefix() {
        let fixture = fixture("win-to-proton");
        let prefix = fixture.root.join("pfx");
        fs::create_dir_all(prefix.join("drive_c/users")).unwrap();

        let mut roots = windows_roots(&fixture.root.join("Documents"));
        roots.add_prefix(&prefix);

        let classified = roots
            .classify(Path::new("C:/Users/me/Documents/My Games/Hades"))
            .expect("a native Windows path is classified");
        assert_eq!(classified.root, RootKind::Documents);
        assert_eq!(classified.relative, "My Games/Hades");
        assert_eq!(classified.layout, Layout::Windows);

        let identity = SaveIdentity::new(9, classified.root, &classified.relative).unwrap();
        let serialized = identity.serialized();
        let fixture_root = fixture.root.to_string_lossy().to_string();
        assert!(!serialized.contains(&fixture_root));
        assert!(!serialized.contains("steamuser"));
        let candidates = roots.candidates(&identity);
        assert_eq!(
            candidates,
            vec![
                fixture.root.join("Documents/My Games/Hades"),
                prefix.join("drive_c/users/steamuser/Documents/My Games/Hades"),
            ]
        );
    }

    #[test]
    fn proton_documents_translate_to_windows_documents() {
        let fixture = fixture("proton-to-win");
        let prefix = fixture.root.join("pfx");
        let save = prefix.join("drive_c/users/steamuser/Documents/My Games/Hades");
        fs::create_dir_all(&save).unwrap();
        fs::write(save.join("profile.sav"), b"save").unwrap();

        let mut roots = SaveRoots::default();
        roots.add_prefix(&prefix);

        let classified = roots.classify(&save).expect("proton path is classified");
        assert_eq!(classified.root, RootKind::Documents);
        assert_eq!(classified.relative, "My Games/Hades");
        assert_eq!(classified.layout, Layout::Proton);

        let identity = SaveIdentity::new(9, classified.root, &classified.relative).unwrap();
        let mut target = SaveRoots::default();
        target.add(
            RootKind::Documents,
            "C:/Users/me/Documents",
            Layout::Windows,
        );
        assert_eq!(
            target.candidates(&identity),
            vec![PathBuf::from("C:/Users/me/Documents/My Games/Hades")]
        );
    }

    #[test]
    fn saved_games_and_appdata_roots_map_both_ways() {
        let cases = [
            (
                "C:/Users/me/Saved Games/Hades",
                RootKind::SavedGames,
                "drive_c/users/steamuser/Saved Games/Hades",
            ),
            (
                "C:/Users/me/AppData/Roaming/Studio/Hades",
                RootKind::RoamingAppData,
                "drive_c/users/steamuser/AppData/Roaming/Studio/Hades",
            ),
            (
                "C:/Users/me/AppData/Local/Studio/Hades",
                RootKind::LocalAppData,
                "drive_c/users/steamuser/AppData/Local/Studio/Hades",
            ),
            (
                "C:/Users/me/AppData/LocalLow/Studio/Hades",
                RootKind::LocalAppDataLow,
                "drive_c/users/steamuser/AppData/LocalLow/Studio/Hades",
            ),
            (
                "C:/ProgramData/Studio/Hades",
                RootKind::ProgramData,
                "drive_c/ProgramData/Studio/Hades",
            ),
            (
                "C:/Users/Public/Documents/Hades",
                RootKind::Public,
                "drive_c/users/Public/Documents/Hades",
            ),
        ];
        for (windows, kind, under_prefix) in cases {
            let classified = classify_windows_text(windows).expect("classified");
            assert_eq!(classified.root, kind, "{windows}");
            let relative = classified.relative.clone();

            let identity = SaveIdentity::new(3, kind, &relative).unwrap();
            let fixture = fixture("kinds");
            let prefix = fixture.root.join("pfx");
            let mut roots = SaveRoots::default();
            roots.add_prefix(&prefix);
            assert_eq!(
                roots.candidates(&identity),
                vec![prefix.join(under_prefix)],
                "{windows}"
            );
        }
    }

    #[test]
    fn one_drive_documents_redirection_is_documents() {
        let classified =
            classify_windows_text("C:/Users/me/OneDrive - Studio/Documents/My Games/Hades")
                .expect("redirection is honoured");
        assert_eq!(classified.root, RootKind::Documents);
        assert_eq!(classified.relative, "My Games/Hades");

        let plain = classify_windows_text("C:/Users/me/OneDrive/Documents/Hades").unwrap();
        assert_eq!(plain.root, RootKind::Documents);
        assert_eq!(plain.relative, "Hades");
    }

    #[test]
    fn case_only_differences_are_the_same_identity_and_use_the_on_disk_casing() {
        let fixture = fixture("case");
        let documents = fixture.root.join("Documents");
        fs::create_dir_all(documents.join("My Games/Hades")).unwrap();
        fs::write(documents.join("My Games/Hades/Profile.sav"), b"save").unwrap();

        let roots = windows_roots(&documents);
        let upper = roots
            .classify(&documents.join("My Games/Hades/Profile.sav"))
            .unwrap();
        let lower = roots
            .classify(&documents.join("my games/hades/profile.sav"))
            .unwrap();
        let upper_identity = SaveIdentity::new(1, upper.root, &upper.relative).unwrap();
        let lower_identity = SaveIdentity::new(1, lower.root, &lower.relative).unwrap();
        assert_ne!(upper_identity.relative, lower_identity.relative);
        assert_eq!(upper_identity.key(), lower_identity.key());

        // Resolution reads the casing that exists on disk, not the identity.
        assert_eq!(
            roots.candidates(&lower_identity),
            vec![documents.join("My Games/Hades/Profile.sav")]
        );
    }

    #[test]
    fn ambiguous_mapping_is_refused_not_picked() {
        let fixture = fixture("ambiguous");
        let first = fixture.root.join("prefix-a");
        let second = fixture.root.join("prefix-b");
        for prefix in [&first, &second] {
            let folder = prefix.join("drive_c/users/steamuser/Documents/Hades");
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("profile.sav"), b"same-save").unwrap();
        }
        let mut roots = SaveRoots::default();
        roots.add_prefix(&first);
        roots.add_prefix(&second);
        let identity = SaveIdentity::new(5, RootKind::Documents, "Hades").unwrap();
        match plan_mapping(&identity, &roots, None) {
            MappingPlan::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn validation_keeps_the_single_matching_pair() {
        let fixture = fixture("validated");
        let source = fixture.root.join("windows/Hades");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("profile.sav"), vec![7u8; 2048]).unwrap();

        let good = fixture.root.join("prefix-good");
        let bad = fixture.root.join("prefix-bad");
        fs::create_dir_all(good.join("drive_c/users/steamuser/Documents/Hades")).unwrap();
        fs::write(
            good.join("drive_c/users/steamuser/Documents/Hades/profile.sav"),
            vec![1u8; 2048],
        )
        .unwrap();
        fs::create_dir_all(bad.join("drive_c/users/steamuser/Documents/Hades")).unwrap();
        fs::write(
            bad.join("drive_c/users/steamuser/Documents/Hades/other.sav"),
            vec![2u8; 2048],
        )
        .unwrap();

        let mut roots = SaveRoots::default();
        roots.add_prefix(&good);
        roots.add_prefix(&bad);
        let identity = SaveIdentity::new(5, RootKind::Documents, "Hades").unwrap();
        match plan_mapping(&identity, &roots, Some(&source)) {
            MappingPlan::Resolved { path } => {
                assert_eq!(path, good.join("drive_c/users/steamuser/Documents/Hades"));
            }
            other => panic!("expected a validated mapping, got {other:?}"),
        }
    }

    #[test]
    fn pair_validation_rejects_names_extensions_and_implausible_sizes() {
        let fixture = fixture("verdicts");
        let source = fixture.root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("profile.sav"), vec![0u8; 1024]).unwrap();
        let roots = SaveRoots::default();
        let identity = SaveIdentity::new(1, RootKind::Documents, "Hades").unwrap();

        let same = fixture.root.join("same");
        fs::create_dir_all(&same).unwrap();
        fs::write(same.join("profile.sav"), vec![3u8; 1000]).unwrap();
        assert_eq!(
            pair_verdict(&roots, &identity, &source, &same),
            PairVerdict::Trusted
        );

        let names = fixture.root.join("names");
        fs::create_dir_all(&names).unwrap();
        fs::write(names.join("other.sav"), vec![3u8; 1000]).unwrap();
        assert!(matches!(
            pair_verdict(&roots, &identity, &source, &names),
            PairVerdict::FileNamesDiffer { .. }
        ));

        let kinds = fixture.root.join("kinds");
        fs::create_dir_all(&kinds).unwrap();
        fs::write(kinds.join("profile.ini"), vec![3u8; 1000]).unwrap();
        assert_eq!(
            pair_verdict(&roots, &identity, &source, &kinds),
            PairVerdict::SignatureDiffers
        );

        let huge = fixture.root.join("huge");
        fs::create_dir_all(&huge).unwrap();
        fs::write(huge.join("profile.sav"), vec![3u8; 128 * 1024]).unwrap();
        assert!(matches!(
            pair_verdict(&roots, &identity, &source, &huge),
            PairVerdict::ImplausibleSize { .. }
        ));
    }

    #[test]
    fn crlf_and_lf_normalise_to_the_same_bytes() {
        assert_eq!(
            normalize_text_bytes(b"slot=1\r\nname=Hades\r\n").unwrap(),
            b"slot=1\nname=Hades\n"
        );
        assert!(normalize_text_bytes(b"no line endings").is_none());
        assert!(normalize_text_bytes(b"binary\0\r\n").is_none());
        assert!(normalize_text_bytes(b"\xff\r\n").is_none());
        assert!(is_text_save_name("settings.INI"));
        assert!(!is_text_save_name("profile.sav"));
    }

    #[test]
    fn unix_only_roots_are_unresolvable_on_a_windows_machine() {
        let fixture = fixture("one-platform");
        let mut roots = SaveRoots::default();
        roots.add(
            RootKind::Documents,
            fixture.root.join("Documents"),
            Layout::Windows,
        );
        let identity = SaveIdentity::new(1, RootKind::XdgData, "Hades").unwrap();
        assert!(!roots.resolvable(&identity));
        assert!(roots.candidates(&identity).is_empty());
    }

    #[test]
    fn mappings_are_deduplicated_by_game_and_identity() {
        let mut mappings = vec![
            SaveIdentityMapping {
                game_id: 1,
                identity: "ludex-save:1/1/documents/Hades".to_string(),
                path: " /a ".to_string(),
            },
            SaveIdentityMapping {
                game_id: 1,
                identity: "ludex-save:1/1/documents/hades".to_string(),
                path: "/b".to_string(),
            },
            SaveIdentityMapping {
                game_id: 2,
                identity: "ludex-save:1/2/documents/Hades".to_string(),
                path: "/c".to_string(),
            },
        ];
        normalize_mappings(&mut mappings);
        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[0].path, "/a");
        assert_eq!(mappings[1].game_id, 2);
    }
}
