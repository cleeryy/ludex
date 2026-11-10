//! GameVault database import (#6).
//!
//! GameVault keeps its library in PostgreSQL. Ludex never reads that database
//! directly: [`scripts/gamevault-export.sql`][script] turns it into a single
//! versioned JSON document and this module only consumes that contract. That
//! keeps the schema assumptions in one documented place and makes the dump
//! reviewable (and optionally encrypted) before anything is imported.
//!
//! [script]: https://github.com/cleeryy/ludex/blob/main/scripts/gamevault-export.sql
//!
//! # What is imported
//!
//! - **Accounts**: matched by username (case-insensitive). Unknown accounts are
//!   created activated, with a random temporary password; GameVault password
//!   hashes are never reused (different scheme). Roles map conservatively:
//!   `admin` → admin, `editor` → editor, everything else → user.
//! - **Games**: matched to an existing library, never created. A GameVault game
//!   is resolved by relative archive path first, then by normalised title plus
//!   archive size. Ambiguity is reported, never guessed.
//! - **Metadata edits**: written into the existing `games` columns (title,
//!   description, artwork *paths*, lists, rating, provider mapping, launch and
//!   installer fields). Columns the dump does not mention are left alone.
//! - **Progress, bookmarks and news**: upserted per account and game.
//!
//! Everything is idempotent: running the same dump twice neither duplicates
//! accounts, progress, bookmarks nor news, and does not reset passwords.
//!
//! # Progress state mapping
//!
//! | GameVault           | Ludex      | Rationale                                    |
//! |---------------------|------------|----------------------------------------------|
//! | `UNPLAYED`          | `unplayed` | same meaning                                 |
//! | `PLAYING`           | `playing`  | same meaning                                 |
//! | `COMPLETED`         | `completed`| same meaning                                 |
//! | `ABORTED_TEMPORARY` | `onHold`   | paused, intends to resume                    |
//! | `ABORTED_PERMANENT` | `abandoned`| dropped for good                             |
//! | `INFINITE`          | `playing`  | endless games are actively played            |
//!
//! # Safety rules
//!
//! - The dump file is opened read-only and never modified.
//! - IDs from GameVault are only used to join sections of the same dump; local
//!   rows always get fresh IDs.
//! - Temporary passwords are returned to the caller exactly once for accounts
//!   created by this run. They are never logged, serialised in the report or
//!   persisted in plaintext. A dry run generates none.
//! - A dry run performs no writes at all.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ludex_core::models::{ProgressState, Role};
use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, SqlitePool};

use crate::config::Config;
use crate::db;

/// Identifier written in the dump's `format` field.
pub const DUMP_FORMAT: &str = "gamevault-export";
/// Contract version understood by this build.
pub const DUMP_VERSION: u32 = 1;
/// Refuse absurd dumps instead of exhausting memory on a wrong file.
const MAX_DUMP_BYTES: u64 = 512 * 1024 * 1024;
/// Length of a generated temporary password.
const TEMP_PASSWORD_LEN: usize = 24;

/// Options for [`run`].
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// JSON document produced by `scripts/gamevault-export.sql`.
    pub dump: PathBuf,
    /// Report the plan without writing anything.
    pub dry_run: bool,
    /// Server configuration: database URL and library media directory.
    pub config: Config,
}

/// How a GameVault game was matched to a Ludex game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchMethod {
    /// Relative archive path, exact.
    FilePath,
    /// Relative archive path, case-insensitive fallback.
    FilePathIgnoreCase,
    /// Normalised title plus at least one identical archive size.
    TitleAndSize,
}

impl MatchMethod {
    /// Stable label used in the human-readable report.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FilePath => "path",
            Self::FilePathIgnoreCase => "path (case-insensitive)",
            Self::TitleAndSize => "title + size",
        }
    }
}

/// Outcome of matching one GameVault game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchStatus {
    /// Resolved to a Ludex game; metadata and progress were applied.
    Matched,
    /// Not resolved, or resolved ambiguously; nothing was applied.
    Unmatched,
    /// Carries no file path or archive size, so there is nothing to match on.
    Ignored,
}

/// One row of the dry-run/import game report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameOutcome {
    /// GameVault game id (join key inside the dump only).
    pub gamevault_id: i64,
    /// GameVault title, for human review.
    pub title: String,
    /// Match result.
    pub status: MatchStatus,
    /// Matching strategy, when matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<MatchMethod>,
    /// Ludex game id, when matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ludex_game_id: Option<i64>,
    /// Why a game was unmatched or ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One row of the account report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountOutcome {
    /// GameVault user id (join key inside the dump only).
    pub gamevault_id: i64,
    /// Login name on the Ludex side.
    pub username: String,
    /// Role assigned to (or already held by) the account.
    pub role: String,
    /// Whether this run created the account.
    pub created: bool,
}

/// Machine-readable result of [`run`].
///
/// All counters describe the work applied by a real run, or the work that
/// *would* be applied by a dry run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Whether the run was a dry run.
    pub dry_run: bool,
    /// Dump path as given on the command line.
    pub dump: String,
    /// Format identifier read from the dump.
    pub dump_format: String,
    /// Contract version read from the dump.
    pub dump_version: u32,
    /// GameVault games resolved to a Ludex game.
    pub matched: usize,
    /// GameVault games with no confident counterpart.
    pub unmatched: usize,
    /// GameVault games with no path or size to match on (skipped).
    pub ignored: usize,
    /// Accounts created by this run.
    pub users_created: usize,
    /// Dump users mapped to an account that already existed.
    pub users_matched: usize,
    /// Progress rows upserted (or planned in a dry run).
    pub progress_imported: usize,
    /// Bookmarks inserted (or planned in a dry run).
    pub bookmarks_imported: usize,
    /// News entries inserted (or planned in a dry run).
    pub news_imported: usize,
    /// Games whose metadata changed.
    pub metadata_updated: usize,
    /// Everything the operator should review.
    pub warnings: Vec<String>,
    /// Per-game rows: matched, unmatched and ignored.
    pub games: Vec<GameOutcome>,
    /// Per-account rows.
    pub accounts: Vec<AccountOutcome>,
}

/// Temporary credential for an account created by this run.
///
/// The password is printed once by the CLI and never stored: there is no
/// `Serialize` implementation and `Debug` redacts it.
#[derive(Clone, PartialEq, Eq)]
pub struct TempCredential {
    /// Account login name.
    pub username: String,
    /// Random temporary password.
    pub password: String,
}

impl fmt::Debug for TempCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TempCredential")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Result of [`run`]: the report plus credentials shown exactly once.
#[derive(Debug)]
pub struct ImportOutcome {
    /// Counters and per-row outcomes.
    pub report: ImportReport,
    /// Temporary passwords for accounts created by this run. Always empty in a
    /// dry run.
    pub credentials: Vec<TempCredential>,
}

/// Imports one GameVault dump into the configured Ludex database.
///
/// # Errors
///
/// Fails when the dump is missing, larger than [`MAX_DUMP_BYTES`], not valid
/// JSON, not a `gamevault-export` document, or from a newer contract version,
/// and on any database error. Unmatched games and skipped rows never fail the
/// run; they are reported as warnings.
pub async fn run(options: ImportOptions) -> Result<ImportOutcome> {
    let dump = read_dump(&options.dump)?;
    let pool = db::connect(&options.config.database_url)
        .await
        .with_context(|| format!("open database {}", options.config.database_url))?;
    crate::roots::bootstrap(&pool, &options.config).await?;

    let index = LibraryIndex::load(&pool).await?;
    let mut warnings = Vec::new();
    let games = resolve_games(&dump, &index);
    let mut mapping: HashMap<i64, i64> = HashMap::new();
    for outcome in &games {
        if let Some(game_id) = outcome.ludex_game_id {
            if let Some(previous) = mapping.insert(outcome.gamevault_id, game_id) {
                if previous != game_id {
                    warnings.push(format!(
                        "GameVault game {} maps to Ludex games {previous} and {game_id}; keeping {game_id}",
                        outcome.gamevault_id
                    ));
                }
            }
        }
    }
    let titles: HashMap<i64, String> = dump
        .games
        .iter()
        .map(|game| (game.id, game.title.clone()))
        .collect();

    let users = import_users(&pool, &dump, options.dry_run, &mut warnings).await?;
    let progress = plan_progress(&dump, &users.ids, &mapping, &titles, &mut warnings);
    let bookmarks = plan_bookmarks(&dump, &users.ids, &mapping, &titles, &mut warnings);
    let news = plan_news(&dump, &users.ids, &mut warnings);

    let (progress_imported, bookmarks_imported, news_imported, metadata_updated) =
        if options.dry_run {
            let known_bookmarks = existing_bookmarks(&pool).await?;
            let known_news = existing_news(&pool).await?;
            let mut connection = pool.acquire().await?;
            let metadata = plan_metadata(&mut connection, &dump, &mapping, &mut warnings).await?;
            let planned_bookmarks = bookmarks
                .iter()
                .filter(|pair| !known_bookmarks.contains(pair))
                .count();
            let planned_news = news
                .iter()
                .filter(|row| !known_news.contains(&(row.title.clone(), row.created_at.clone())))
                .count();
            (
                progress.len(),
                planned_bookmarks,
                planned_news,
                metadata.len(),
            )
        } else {
            let mut transaction = pool.begin().await.context("begin import transaction")?;
            let metadata = plan_metadata(&mut transaction, &dump, &mapping, &mut warnings).await?;
            let progress_imported = apply_progress(&mut transaction, &progress).await?;
            let bookmarks_imported = apply_bookmarks(&mut transaction, &bookmarks).await?;
            let news_imported = apply_news(&mut transaction, &news).await?;
            let metadata_updated = apply_metadata(&mut transaction, &metadata).await?;
            transaction.commit().await.context("commit import")?;
            (
                progress_imported,
                bookmarks_imported,
                news_imported,
                metadata_updated,
            )
        };

    let artwork_paths = dump
        .games
        .iter()
        .filter(|game| {
            mapping.contains_key(&game.id)
                && (non_empty(&game.cover).is_some() || non_empty(&game.background).is_some())
        })
        .count();
    if artwork_paths > 0 {
        warnings.push(format!(
            "{artwork_paths} games keep GameVault artwork paths; the files are not copied and provider metadata refresh will replace them"
        ));
    }

    let matched = games
        .iter()
        .filter(|outcome| outcome.status == MatchStatus::Matched)
        .count();
    let unmatched = games
        .iter()
        .filter(|outcome| outcome.status == MatchStatus::Unmatched)
        .count();
    let ignored = games
        .iter()
        .filter(|outcome| outcome.status == MatchStatus::Ignored)
        .count();

    let report = ImportReport {
        dry_run: options.dry_run,
        dump: options.dump.display().to_string(),
        dump_format: dump.format.clone(),
        dump_version: dump.version,
        matched,
        unmatched,
        ignored,
        users_created: users.created,
        users_matched: users.matched,
        progress_imported,
        bookmarks_imported,
        news_imported,
        metadata_updated,
        warnings,
        games,
        accounts: users.accounts,
    };
    Ok(ImportOutcome {
        report,
        credentials: users.credentials,
    })
}

// ---------------------------------------------------------------------------
// Dump contract
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dump {
    format: String,
    version: u32,
    #[serde(default)]
    users: Vec<DumpUser>,
    #[serde(default)]
    games: Vec<DumpGame>,
    #[serde(default)]
    progress: Vec<DumpProgress>,
    #[serde(default)]
    bookmarks: Vec<DumpBookmark>,
    #[serde(default)]
    news: Vec<DumpNews>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DumpUser {
    id: i64,
    username: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    activated: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DumpGame {
    id: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    cover: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    genres: Vec<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    developers: Vec<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    publishers: Vec<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    tags: Vec<String>,
    #[serde(default)]
    rating: Option<f64>,
    #[serde(default)]
    early_access: Option<bool>,
    #[serde(default)]
    provider_slug: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    provider_url: Option<String>,
    #[serde(default)]
    launch_executable: Option<String>,
    #[serde(default)]
    launch_parameters: Option<String>,
    #[serde(default)]
    installer_executable: Option<String>,
    #[serde(default)]
    installer_parameters: Option<String>,
    #[serde(default)]
    uninstaller_executable: Option<String>,
    #[serde(default)]
    uninstaller_parameters: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    versions: Vec<DumpVersion>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DumpVersion {
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    size: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DumpProgress {
    user_id: i64,
    game_id: i64,
    #[serde(default)]
    state: String,
    #[serde(default)]
    minutes_played: Option<i64>,
    /// Forward compatibility: newer exports may report seconds instead.
    #[serde(default)]
    seconds_played: Option<i64>,
    #[serde(default)]
    last_played_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DumpBookmark {
    user_id: i64,
    game_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DumpNews {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    author_id: Option<i64>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

/// Treats an explicit JSON `null` like a missing field.
fn null_to_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn read_dump(path: &Path) -> Result<Dump> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("read GameVault dump {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    if metadata.len() > MAX_DUMP_BYTES {
        bail!(
            "{} is larger than {} bytes; refusing to load it",
            path.display(),
            MAX_DUMP_BYTES
        );
    }
    let bytes =
        std::fs::read(path).with_context(|| format!("read GameVault dump {}", path.display()))?;
    let dump: Dump = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "parse {} as GameVault JSON; run scripts/gamevault-export.sql (see docs/MIGRATION.md)",
            path.display()
        )
    })?;
    if dump.format != DUMP_FORMAT {
        bail!(
            "unsupported dump format `{}`; expected `{DUMP_FORMAT}`",
            dump.format
        );
    }
    if dump.version == 0 || dump.version > DUMP_VERSION {
        bail!(
            "unsupported dump version {}; this build understands version {DUMP_VERSION}",
            dump.version
        );
    }
    Ok(dump)
}

// ---------------------------------------------------------------------------
// Game matching
// ---------------------------------------------------------------------------

#[derive(Default)]
struct LibraryIndex {
    path_games: HashMap<String, HashSet<i64>>,
    lower_path_games: HashMap<String, HashSet<i64>>,
    title_games: HashMap<String, HashSet<i64>>,
    game_sizes: HashMap<i64, HashSet<i64>>,
    game_titles: HashMap<i64, String>,
}

impl LibraryIndex {
    async fn load(pool: &SqlitePool) -> Result<Self> {
        let mut index = Self::default();
        let games = sqlx::query_as::<_, (i64, String)>("SELECT id, title FROM games")
            .fetch_all(pool)
            .await
            .context("read games for matching")?;
        for (id, title) in games {
            index
                .title_games
                .entry(ludex_core::normalize_title(&title))
                .or_default()
                .insert(id);
            index.game_titles.insert(id, title);
        }
        let versions = sqlx::query_as::<_, (i64, String, i64)>(
            "SELECT game_id, file_path, size FROM game_versions",
        )
        .fetch_all(pool)
        .await
        .context("read versions for matching")?;
        for (game_id, file_path, size) in versions {
            let path = normalize_path(&file_path);
            index
                .path_games
                .entry(path.clone())
                .or_default()
                .insert(game_id);
            index
                .lower_path_games
                .entry(path.to_lowercase())
                .or_default()
                .insert(game_id);
            if size > 0 {
                index.game_sizes.entry(game_id).or_default().insert(size);
            }
        }
        Ok(index)
    }
}

/// Normalises an archive path for comparison.
///
/// GameVault stores paths relative to its games volume; Ludex does the same, so
/// only separator and prefix differences are removed.
fn normalize_path(path: &str) -> String {
    let unified = path.trim().replace('\\', "/");
    unified
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

fn resolve_games(dump: &Dump, index: &LibraryIndex) -> Vec<GameOutcome> {
    dump.games
        .iter()
        .map(|game| resolve_game(game, index))
        .collect()
}

fn resolve_game(game: &DumpGame, index: &LibraryIndex) -> GameOutcome {
    let mut outcome = GameOutcome {
        gamevault_id: game.id,
        title: game.title.clone(),
        status: MatchStatus::Unmatched,
        method: None,
        ludex_game_id: None,
        detail: None,
    };

    if game.versions.is_empty() {
        outcome.status = MatchStatus::Ignored;
        outcome.detail = Some("no file path or archive size in the dump".to_string());
        return outcome;
    }

    let mut exact: HashSet<i64> = HashSet::new();
    let mut folded: HashSet<i64> = HashSet::new();
    for version in &game.versions {
        let Some(path) = non_empty(&version.file_path) else {
            continue;
        };
        let path = normalize_path(path);
        if let Some(game_ids) = index.path_games.get(&path) {
            exact.extend(game_ids.iter().copied());
        }
        if let Some(game_ids) = index.lower_path_games.get(&path.to_lowercase()) {
            folded.extend(game_ids.iter().copied());
        }
    }

    if let Some(game_id) = single(&exact) {
        return matched(outcome, index, game_id, MatchMethod::FilePath);
    }
    if exact.len() > 1 {
        outcome.detail = Some(format!(
            "several Ludex games share this path: {}",
            describe(index, &exact)
        ));
        return outcome;
    }
    if let Some(game_id) = single(&folded) {
        return matched(outcome, index, game_id, MatchMethod::FilePathIgnoreCase);
    }
    if folded.len() > 1 {
        outcome.detail = Some(format!(
            "several Ludex games share this path (case-insensitive): {}",
            describe(index, &folded)
        ));
        return outcome;
    }

    let normalized_title = ludex_core::normalize_title(&game.title);
    let sizes: HashSet<i64> = game
        .versions
        .iter()
        .filter_map(|version| version.size.filter(|size| *size > 0))
        .collect();
    if normalized_title.is_empty() || sizes.is_empty() {
        outcome.detail = Some("no matching path, title or archive size".to_string());
        return outcome;
    }
    let candidates: HashSet<i64> = index
        .title_games
        .get(&normalized_title)
        .cloned()
        .unwrap_or_default();
    let candidates: HashSet<i64> = candidates
        .into_iter()
        .filter(|game_id| {
            index
                .game_sizes
                .get(game_id)
                .is_some_and(|known| !known.is_disjoint(&sizes))
        })
        .collect();
    match candidates.len() {
        1 => matched(
            outcome,
            index,
            *candidates.iter().next().unwrap_or(&0),
            MatchMethod::TitleAndSize,
        ),
        0 => {
            outcome.detail =
                Some("no Ludex game with this path, title and archive size".to_string());
            outcome
        }
        _ => {
            outcome.detail = Some(format!(
                "title and archive size are ambiguous between {}",
                describe(index, &candidates)
            ));
            outcome
        }
    }
}

fn matched(
    mut outcome: GameOutcome,
    index: &LibraryIndex,
    game_id: i64,
    method: MatchMethod,
) -> GameOutcome {
    outcome.status = MatchStatus::Matched;
    outcome.method = Some(method);
    outcome.ludex_game_id = Some(game_id);
    outcome.detail = index.game_titles.get(&game_id).cloned();
    outcome
}

fn describe(index: &LibraryIndex, ids: &HashSet<i64>) -> String {
    let mut titles: Vec<String> = ids
        .iter()
        .map(|id| {
            index
                .game_titles
                .get(id)
                .cloned()
                .unwrap_or_else(|| format!("game {id}"))
        })
        .collect();
    titles.sort();
    titles.truncate(5);
    titles.join(", ")
}

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalUser {
    /// Account already present, or created by this run.
    Id(i64),
    /// Account that a dry run would create. The synthetic negative id keeps
    /// progress/bookmark/news planning observable without touching the
    /// database (real ids are always positive).
    Planned(i64),
}

struct UserImport {
    ids: HashMap<i64, LocalUser>,
    accounts: Vec<AccountOutcome>,
    credentials: Vec<TempCredential>,
    created: usize,
    matched: usize,
}

async fn import_users(
    pool: &SqlitePool,
    dump: &Dump,
    dry_run: bool,
    warnings: &mut Vec<String>,
) -> Result<UserImport> {
    let existing = sqlx::query_as::<_, (i64, String)>("SELECT id, username FROM users")
        .fetch_all(pool)
        .await
        .context("read existing accounts")?;
    let mut known: HashMap<String, LocalUser> = existing
        .into_iter()
        .map(|(id, username)| (username.to_lowercase(), LocalUser::Id(id)))
        .collect();

    let mut import = UserImport {
        ids: HashMap::new(),
        accounts: Vec::new(),
        credentials: Vec::new(),
        created: 0,
        matched: 0,
    };

    for user in &dump.users {
        let username = user.username.trim();
        if username.is_empty() {
            warnings.push(format!("GameVault user {} has no username", user.id));
            continue;
        }
        if username.chars().count() > 64 || username.chars().any(char::is_control) {
            warnings.push(format!(
                "GameVault user {} has an invalid username (`{username}`); skipped",
                user.id
            ));
            continue;
        }
        let key = username.to_lowercase();
        let role = map_role(user, warnings);

        if let Some(local) = known.get(&key).copied() {
            import.ids.insert(user.id, local);
            import.matched += 1;
            if matches!(local, LocalUser::Planned(_)) {
                import.accounts.push(AccountOutcome {
                    gamevault_id: user.id,
                    username: username.to_string(),
                    role: role.as_str().to_string(),
                    created: true,
                });
            }
            continue;
        }

        if dry_run {
            let planned = LocalUser::Planned(-user.id.max(1));
            known.insert(key, planned);
            import.ids.insert(user.id, planned);
            import.created += 1;
            import.accounts.push(AccountOutcome {
                gamevault_id: user.id,
                username: username.to_string(),
                role: role.as_str().to_string(),
                created: true,
            });
            continue;
        }

        let password = temporary_password();
        let password_hash = crate::auth::hash_password(&password)?;
        let now = db::db_time(db::now());
        let email = non_empty(&user.email);
        let inserted = sqlx::query(
            "INSERT INTO users
                (username, password_hash, email, role, activated, avatar_path, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, NULL, ?, ?)",
        )
        .bind(username)
        .bind(&password_hash)
        .bind(email)
        .bind(role.as_str())
        .bind(i64::from(user.activated))
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await;
        let id = match inserted {
            Ok(result) => result.last_insert_rowid(),
            Err(error) => {
                warnings.push(format!("could not create account `{username}`: {error}"));
                continue;
            }
        };
        known.insert(key, LocalUser::Id(id));
        import.ids.insert(user.id, LocalUser::Id(id));
        import.created += 1;
        import.credentials.push(TempCredential {
            username: username.to_string(),
            password,
        });
        import.accounts.push(AccountOutcome {
            gamevault_id: user.id,
            username: username.to_string(),
            role: role.as_str().to_string(),
            created: true,
        });
    }
    Ok(import)
}

fn map_role(user: &DumpUser, warnings: &mut Vec<String>) -> Role {
    let raw = user
        .role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("admin") => Role::Admin,
        Some("editor") => Role::Editor,
        Some("user") | None => Role::User,
        Some(other) => {
            warnings.push(format!(
                "unknown GameVault role `{other}` for `{}`; using user",
                user.username
            ));
            Role::User
        }
    }
}

/// Random alphanumeric password from the operating system CSPRNG.
fn temporary_password() -> String {
    use rand::distributions::Alphanumeric;
    use rand::Rng;

    let mut rng = rand::rngs::OsRng;
    (0..TEMP_PASSWORD_LEN)
        .map(|_| char::from(rng.sample(Alphanumeric)))
        .collect()
}

// ---------------------------------------------------------------------------
// Progress, bookmarks and news
// ---------------------------------------------------------------------------

struct ProgressPlan {
    user_id: i64,
    game_id: i64,
    state: ProgressState,
    minutes: i64,
    last_played_at: Option<String>,
}

fn plan_progress(
    dump: &Dump,
    users: &HashMap<i64, LocalUser>,
    games: &HashMap<i64, i64>,
    titles: &HashMap<i64, String>,
    warnings: &mut Vec<String>,
) -> Vec<ProgressPlan> {
    let mut plans = Vec::new();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    for row in &dump.progress {
        let Some(user_id) = users.get(&row.user_id).and_then(LocalUser::id) else {
            warnings.push(format!(
                "progress for GameVault user {} skipped: no Ludex account",
                row.user_id
            ));
            continue;
        };
        let Some(game_id) = games.get(&row.game_id).copied() else {
            warnings.push(format!(
                "progress for `{}` skipped: game is not in the Ludex library",
                titles
                    .get(&row.game_id)
                    .map_or_else(|| row.game_id.to_string(), Clone::clone)
            ));
            continue;
        };
        let Some(state) = map_state(&row.state) else {
            warnings.push(format!(
                "progress for `{}` skipped: unknown GameVault state `{}`",
                titles
                    .get(&row.game_id)
                    .map_or_else(|| row.game_id.to_string(), Clone::clone),
                row.state
            ));
            continue;
        };
        if !seen.insert((user_id, game_id)) {
            warnings.push(format!(
                "duplicate progress row for user {user_id} and game {game_id}; keeping the first"
            ));
            continue;
        }
        let mut minutes = row.minutes_played.unwrap_or(0);
        if row.minutes_played.is_none() {
            if let Some(seconds) = row.seconds_played {
                minutes = (seconds.max(0) + 59) / 60;
            }
        }
        if minutes < 0 {
            warnings.push(format!(
                "negative playtime for user {user_id} and game {game_id}; clamped to 0"
            ));
            minutes = 0;
        }
        plans.push(ProgressPlan {
            user_id,
            game_id,
            state,
            minutes,
            last_played_at: row.last_played_at.as_deref().and_then(stored_time),
        });
    }
    plans
}

impl LocalUser {
    fn id(&self) -> Option<i64> {
        match self {
            Self::Id(id) | Self::Planned(id) => Some(*id),
        }
    }
}

fn map_state(raw: &str) -> Option<ProgressState> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "UNPLAYED" => Some(ProgressState::Unplayed),
        "PLAYING" => Some(ProgressState::Playing),
        "COMPLETED" => Some(ProgressState::Completed),
        "ABORTED_TEMPORARY" | "ONHOLD" | "ON_HOLD" => Some(ProgressState::OnHold),
        "ABORTED_PERMANENT" => Some(ProgressState::Abandoned),
        "INFINITE" => Some(ProgressState::Playing),
        "PLAN_TO_PLAY" | "PLANTOPLAY" => Some(ProgressState::PlanToPlay),
        _ => None,
    }
}

async fn apply_progress(connection: &mut SqliteConnection, plan: &[ProgressPlan]) -> Result<usize> {
    let mut applied = 0;
    for row in plan {
        // Idempotency: an unchanged row is not an import. Compute the merged
        // values first so the report counts real changes only.
        let current = sqlx::query_as::<_, (String, i64, Option<String>)>(
            "SELECT state, minutes_played, last_played_at FROM progress
             WHERE user_id = ? AND game_id = ?",
        )
        .bind(row.user_id)
        .bind(row.game_id)
        .fetch_optional(&mut *connection)
        .await
        .context("read existing progress")?;
        let merged_last = match &current {
            Some((_, _, last)) => match (last.as_deref(), row.last_played_at.as_deref()) {
                (_, None) => last.clone(),
                (None, Some(incoming)) => Some(incoming.to_string()),
                (Some(current_last), Some(incoming)) if incoming > current_last => {
                    Some(incoming.to_string())
                }
                (Some(current_last), Some(_)) => Some(current_last.to_string()),
            },
            None => row.last_played_at.clone(),
        };
        if let Some((state, minutes, last_played_at)) = &current {
            let merged_minutes = (*minutes).max(row.minutes);
            if state == row.state.as_str()
                && merged_minutes == *minutes
                && merged_last == *last_played_at
            {
                continue;
            }
        }
        sqlx::query(
            "INSERT INTO progress (user_id, game_id, state, minutes_played, last_played_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(user_id, game_id) DO UPDATE SET
                state = excluded.state,
                minutes_played = MAX(progress.minutes_played, excluded.minutes_played),
                last_played_at = excluded.last_played_at",
        )
        .bind(row.user_id)
        .bind(row.game_id)
        .bind(row.state.as_str())
        .bind(row.minutes)
        .bind(merged_last.as_deref())
        .execute(&mut *connection)
        .await
        .context("import progress")?;
        applied += 1;
    }
    Ok(applied)
}

fn plan_bookmarks(
    dump: &Dump,
    users: &HashMap<i64, LocalUser>,
    games: &HashMap<i64, i64>,
    titles: &HashMap<i64, String>,
    warnings: &mut Vec<String>,
) -> Vec<(i64, i64)> {
    let mut plans = Vec::new();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    for row in &dump.bookmarks {
        let Some(user_id) = users.get(&row.user_id).and_then(LocalUser::id) else {
            warnings.push(format!(
                "bookmark for GameVault user {} skipped: no Ludex account",
                row.user_id
            ));
            continue;
        };
        let Some(game_id) = games.get(&row.game_id).copied() else {
            warnings.push(format!(
                "bookmark for `{}` skipped: game is not in the Ludex library",
                titles
                    .get(&row.game_id)
                    .map_or_else(|| row.game_id.to_string(), Clone::clone)
            ));
            continue;
        };
        if seen.insert((user_id, game_id)) {
            plans.push((user_id, game_id));
        }
    }
    plans
}

async fn apply_bookmarks(connection: &mut SqliteConnection, plan: &[(i64, i64)]) -> Result<usize> {
    let mut applied = 0;
    for (user_id, game_id) in plan {
        let result = sqlx::query(
            "INSERT INTO bookmarks (user_id, game_id) VALUES (?, ?)
             ON CONFLICT(user_id, game_id) DO NOTHING",
        )
        .bind(user_id)
        .bind(game_id)
        .execute(&mut *connection)
        .await
        .context("import bookmark")?;
        applied += usize::try_from(result.rows_affected()).unwrap_or(0);
    }
    Ok(applied)
}

struct NewsPlan {
    title: String,
    body: String,
    author_id: Option<i64>,
    created_at: String,
    updated_at: String,
}

fn plan_news(
    dump: &Dump,
    users: &HashMap<i64, LocalUser>,
    warnings: &mut Vec<String>,
) -> Vec<NewsPlan> {
    let mut plans = Vec::new();
    let now = db::db_time(db::now());
    for row in &dump.news {
        let title = row.title.trim();
        if title.is_empty() {
            warnings.push("news entry without a title skipped".to_string());
            continue;
        }
        let body = row.body.trim();
        if body.is_empty() {
            warnings.push(format!("news entry `{title}` has no body; skipped"));
            continue;
        }
        let author_id = row
            .author_id
            .and_then(|id| users.get(&id).and_then(LocalUser::id));
        if row.author_id.is_some() && author_id.is_none() {
            warnings.push(format!(
                "news entry `{title}` keeps no author: the GameVault author has no Ludex account"
            ));
        }
        let created_at = row
            .created_at
            .as_deref()
            .and_then(stored_time)
            .unwrap_or_else(|| now.clone());
        let updated_at = row
            .updated_at
            .as_deref()
            .and_then(stored_time)
            .unwrap_or_else(|| created_at.clone());
        plans.push(NewsPlan {
            title: title.to_string(),
            body: body.to_string(),
            author_id,
            created_at,
            updated_at,
        });
    }
    plans
}

async fn apply_news(connection: &mut SqliteConnection, plan: &[NewsPlan]) -> Result<usize> {
    let mut applied = 0;
    for row in plan {
        let result = sqlx::query(
            "INSERT INTO news (title, body, author_id, created_at, updated_at)
             SELECT ?, ?, ?, ?, ?
             WHERE NOT EXISTS (
                 SELECT 1 FROM news WHERE title = ? AND created_at = ?
             )",
        )
        .bind(&row.title)
        .bind(&row.body)
        .bind(row.author_id)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .bind(&row.title)
        .bind(&row.created_at)
        .execute(&mut *connection)
        .await
        .context("import news")?;
        applied += usize::try_from(result.rows_affected()).unwrap_or(0);
    }
    Ok(applied)
}

async fn existing_bookmarks(pool: &SqlitePool) -> Result<HashSet<(i64, i64)>> {
    let rows = sqlx::query_as::<_, (i64, i64)>("SELECT user_id, game_id FROM bookmarks")
        .fetch_all(pool)
        .await
        .context("read bookmarks")?;
    Ok(rows.into_iter().collect())
}

async fn existing_news(pool: &SqlitePool) -> Result<HashSet<(String, String)>> {
    let rows = sqlx::query_as::<_, (String, String)>("SELECT title, created_at FROM news")
        .fetch_all(pool)
        .await
        .context("read news")?;
    Ok(rows.into_iter().collect())
}

/// Parses a GameVault timestamp (`2026-01-02T03:04:05Z`, with or without
/// fractional seconds) into the storage format.
fn stored_time(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    db::parse_date(trimmed).map(db::db_time)
}

// ---------------------------------------------------------------------------
// Metadata overrides
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow, PartialEq)]
struct MetadataRow {
    title: String,
    sort_title: String,
    description: Option<String>,
    release_date: Option<String>,
    cover_path: Option<String>,
    background_path: Option<String>,
    genres_json: String,
    developers_json: String,
    publishers_json: String,
    tags_json: String,
    rating: Option<f64>,
    early_access: i64,
    provider_slug: Option<String>,
    provider_id: Option<String>,
    provider_url: Option<String>,
    launch_executable: Option<String>,
    launch_parameters: Option<String>,
    installer_executable: Option<String>,
    installer_parameters: Option<String>,
    uninstaller_executable: Option<String>,
    uninstaller_parameters: Option<String>,
}

async fn plan_metadata(
    connection: &mut SqliteConnection,
    dump: &Dump,
    games: &HashMap<i64, i64>,
    warnings: &mut Vec<String>,
) -> Result<Vec<(i64, MetadataRow)>> {
    let mut changes = Vec::new();
    for game in &dump.games {
        let Some(game_id) = games.get(&game.id).copied() else {
            continue;
        };
        let current = sqlx::query_as::<_, MetadataRow>(
            "SELECT title, sort_title, description, release_date, cover_path, background_path,
                    genres_json, developers_json, publishers_json, tags_json, rating,
                    early_access, provider_slug, provider_id, provider_url, launch_executable,
                    launch_parameters, installer_executable, installer_parameters,
                    uninstaller_executable, uninstaller_parameters
             FROM games WHERE id = ?",
        )
        .bind(game_id)
        .fetch_optional(&mut *connection)
        .await
        .context("read game metadata")?;
        let Some(current) = current else {
            warnings.push(format!(
                "game `{}` disappeared before its metadata could be imported",
                game.title
            ));
            continue;
        };
        if let Some(merged) = merge_metadata(&current, game, warnings) {
            changes.push((game_id, merged));
        }
    }
    Ok(changes)
}

fn merge_metadata(
    current: &MetadataRow,
    game: &DumpGame,
    warnings: &mut Vec<String>,
) -> Option<MetadataRow> {
    let mut merged = current.clone();
    let title = game.title.trim();
    if !title.is_empty() && title != current.title {
        merged.title = title.to_string();
        merged.sort_title = sort_title(title);
    }
    if let Some(description) = non_empty(&game.description) {
        merged.description = Some(description.to_string());
    }
    if let Some(raw) = non_empty(&game.release_date) {
        match stored_time(raw) {
            Some(value) => merged.release_date = Some(value),
            None => warnings.push(format!(
                "game `{}` has an invalid release date `{raw}`; kept the local value",
                game.title
            )),
        }
    }
    if let Some(cover) = non_empty(&game.cover) {
        merged.cover_path = Some(cover.to_string());
    }
    if let Some(background) = non_empty(&game.background) {
        merged.background_path = Some(background.to_string());
    }
    for (values, target) in [
        (&game.genres, &mut merged.genres_json),
        (&game.developers, &mut merged.developers_json),
        (&game.publishers, &mut merged.publishers_json),
        (&game.tags, &mut merged.tags_json),
    ] {
        let items = cleaned_list(values);
        if !items.is_empty() {
            *target = db::dump_json_list(&items);
        }
    }
    if let Some(rating) = game
        .rating
        .filter(|value| value.is_finite() && *value > 0.0)
    {
        merged.rating = Some(rating);
    }
    // GameVault defaults `early_access` to false, so only a positive value is
    // trusted: it must not erase Ludex's filename-based detection.
    if game.early_access == Some(true) {
        merged.early_access = 1;
    }
    if let Some(value) = non_empty(&game.provider_slug) {
        merged.provider_slug = Some(value.to_string());
    }
    if let Some(value) = non_empty(&game.provider_id) {
        merged.provider_id = Some(value.to_string());
    }
    if let Some(value) = non_empty(&game.provider_url) {
        merged.provider_url = Some(value.to_string());
    }
    merged.launch_executable = non_empty(&game.launch_executable).map(str::to_string);
    merged.launch_parameters = non_empty(&game.launch_parameters).map(str::to_string);
    merged.installer_executable = non_empty(&game.installer_executable).map(str::to_string);
    merged.installer_parameters = non_empty(&game.installer_parameters).map(str::to_string);
    merged.uninstaller_executable = non_empty(&game.uninstaller_executable).map(str::to_string);
    merged.uninstaller_parameters = non_empty(&game.uninstaller_parameters).map(str::to_string);

    if merged == *current {
        None
    } else {
        Some(merged)
    }
}

async fn apply_metadata(
    connection: &mut SqliteConnection,
    changes: &[(i64, MetadataRow)],
) -> Result<usize> {
    let now = db::db_time(db::now());
    for (game_id, row) in changes {
        sqlx::query(
            "UPDATE games SET title = ?, sort_title = ?, description = ?, release_date = ?,
                    cover_path = ?, background_path = ?, genres_json = ?, developers_json = ?,
                    publishers_json = ?, tags_json = ?, rating = ?, early_access = ?,
                    provider_slug = ?, provider_id = ?, provider_url = ?,
                    launch_executable = ?, launch_parameters = ?, installer_executable = ?,
                    installer_parameters = ?, uninstaller_executable = ?,
                    uninstaller_parameters = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(&row.title)
        .bind(&row.sort_title)
        .bind(&row.description)
        .bind(&row.release_date)
        .bind(&row.cover_path)
        .bind(&row.background_path)
        .bind(&row.genres_json)
        .bind(&row.developers_json)
        .bind(&row.publishers_json)
        .bind(&row.tags_json)
        .bind(row.rating)
        .bind(row.early_access)
        .bind(&row.provider_slug)
        .bind(&row.provider_id)
        .bind(&row.provider_url)
        .bind(&row.launch_executable)
        .bind(&row.launch_parameters)
        .bind(&row.installer_executable)
        .bind(&row.installer_parameters)
        .bind(&row.uninstaller_executable)
        .bind(&row.uninstaller_parameters)
        .bind(&now)
        .bind(game_id)
        .execute(&mut *connection)
        .await
        .context("import game metadata")?;
    }
    Ok(changes.len())
}

/// Mirrors the private `ludex_core::naming::sort_title`: strips a leading
/// English article so imported titles sort like scanned ones.
fn sort_title(title: &str) -> String {
    let trimmed = title.trim_start();
    for article in ["the ", "an ", "a "] {
        let Some(prefix) = trimmed.get(..article.len()) else {
            continue;
        };
        if prefix.eq_ignore_ascii_case(article) {
            let rest = trimmed[article.len()..].trim_start();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    trimmed.to_string()
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// Returns the only element of `ids`, or `None` when the set is empty or
/// ambiguous. Matching never guesses between two candidates.
fn single(ids: &HashSet<i64>) -> Option<i64> {
    if ids.len() == 1 {
        ids.iter().next().copied()
    } else {
        None
    }
}

fn cleaned_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_normalise_separators_and_prefixes() {
        assert_eq!(normalize_path(r".\Games\a.zip"), "Games/a.zip");
        assert_eq!(normalize_path("/games//a.zip"), "games//a.zip");
        assert_eq!(normalize_path("  a.zip  "), "a.zip");
    }

    #[test]
    fn gamevault_states_map_conservatively() {
        assert_eq!(map_state("UNPLAYED"), Some(ProgressState::Unplayed));
        assert_eq!(map_state("PLAYING"), Some(ProgressState::Playing));
        assert_eq!(map_state("COMPLETED"), Some(ProgressState::Completed));
        assert_eq!(map_state("ABORTED_TEMPORARY"), Some(ProgressState::OnHold));
        assert_eq!(
            map_state("ABORTED_PERMANENT"),
            Some(ProgressState::Abandoned)
        );
        assert_eq!(map_state("INFINITE"), Some(ProgressState::Playing));
        assert_eq!(map_state("DROPPED"), None);
    }

    #[test]
    fn sort_titles_strip_english_articles() {
        assert_eq!(sort_title("The Roottrees are Dead"), "Roottrees are Dead");
        assert_eq!(sort_title("A Short Hike"), "Short Hike");
        assert_eq!(sort_title("Anodyne"), "Anodyne");
    }

    #[test]
    fn temporary_passwords_are_long_and_alphanumeric() {
        let first = temporary_password();
        assert_eq!(first.len(), TEMP_PASSWORD_LEN);
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(first, temporary_password());
    }

    #[test]
    fn credentials_never_debug_their_password() {
        let credential = TempCredential {
            username: "alice".to_string(),
            password: "supersecret".to_string(),
        };
        assert!(!format!("{credential:?}").contains("supersecret"));
    }
}
