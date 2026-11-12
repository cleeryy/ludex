//! Shareable collection links (#173).
//!
//! A collection share link is a bearer capability, using the same rules as the
//! game share links in [`crate::share`]: only the SHA-256 of the token is
//! stored, the plaintext URL is returned to the owner once, links expire
//! optionally, can be password-protected, are rate-limited and are always
//! revocable. A private collection can never be read through a token, even if
//! a link created while it was unlisted still exists.
//!
//! The public payload is metadata only: game ids, titles, covers, release
//! years and genres. It never joins progress, bookmarks, play sessions or
//! library roots, so file paths and account data cannot leak.

use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::auth;
use crate::collections::{self, Candidate};
use crate::db;
use crate::db::Database;
use crate::error::ApiError;
use crate::media;
use crate::models::{CollectionKind, CollectionVisibility, SmartRules};
use crate::share::ShareDenial;
use crate::sql::{bind_query_as, Bind};

/// Prefix of every collection share token.
pub const TOKEN_PREFIX: &str = "lsc_";
/// Random bytes behind one token.
const TOKEN_BYTES: usize = 32;

/// Body of `POST /api/collections/{id}/share`.
#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateCollectionShareRequest {
    /// Lifetime in hours (1..=8760); `None` never expires.
    #[serde(default)]
    pub expires_in_hours: Option<i64>,
    /// Optional access password (at least 4 characters).
    #[serde(default)]
    pub password: Option<String>,
    /// Owner-facing label, never shown on the public page.
    #[serde(default)]
    pub label: Option<String>,
}

/// Body of `POST /api/share/collection/{token}/access`.
#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionShareAccessRequest {
    /// Password when the link is protected.
    #[serde(default)]
    pub password: Option<String>,
}

/// Body of `POST /api/collections/import`.
#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportCollectionRequest {
    /// Public collection to copy. Exactly one of `sourceId` and `token`.
    #[serde(default)]
    pub source_id: Option<i64>,
    /// Share token for an unlisted or public collection.
    #[serde(default)]
    pub token: Option<String>,
    /// Password when the share token is protected.
    #[serde(default)]
    pub password: Option<String>,
    /// Target name; defaults to the source name. Names are unique per account.
    #[serde(default)]
    pub name: Option<String>,
}

/// Response of `POST /api/collections/import`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportCollectionResponse {
    /// The collection now in the caller's account.
    pub collection: crate::models::Collection,
    /// `false` when the import was already present and missing games were merged.
    pub created: bool,
    /// Manual rows inserted by this request.
    pub added: i64,
    /// Source games that were already present or could not be copied.
    pub skipped: i64,
}

/// Owner-facing view of a collection share link. The plaintext token is only
/// present in the creation response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionShareLink {
    /// Row id.
    pub id: i64,
    /// Plaintext token; only present on creation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Shared collection.
    pub collection_id: i64,
    /// Collection name, for the management list.
    pub collection_name: String,
    /// Owner-facing label.
    pub label: Option<String>,
    /// RFC 3339 expiry, `None` when the link never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the public page needs a password to show the games.
    pub has_password: bool,
    /// Public page views so far.
    pub view_count: i64,
    /// Revocation timestamp.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Public collection metadata. Account display names appear only when the
/// owner opted into a public profile.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharedCollection {
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Manual or smart.
    pub kind: CollectionKind,
    /// Number of games on the shelf.
    pub game_count: i64,
    /// Public owner display name, when the profile is public.
    pub owner: Option<SharedCollectionOwner>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Owner label shown on a public collection page.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharedCollectionOwner {
    /// Display name (or username when no display name is set).
    pub display_name: String,
}

/// One game on a public collection page. Metadata only.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharedCollectionGame {
    /// Game id.
    pub id: i64,
    /// Display title.
    pub title: String,
    /// Cover image URL, when cached.
    pub cover_url: Option<String>,
    /// Release year, when known.
    pub release_year: Option<i32>,
    /// Genre labels.
    pub genres: Vec<String>,
}

/// Response of `GET /api/share/collection/{token}` and its access route.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionShareView {
    /// Public collection metadata.
    pub collection: SharedCollection,
    /// Games on the shelf; empty when a password is required and not supplied.
    pub games: Vec<SharedCollectionGame>,
    /// Share expiry, `None` when it never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the access route needs a password to reveal the games.
    pub has_password: bool,
}

/// Row of `collection_shares` joined with its collection.
#[derive(Debug, sqlx::FromRow)]
pub struct CollectionShareRow {
    /// Row id.
    pub id: i64,
    /// Shared collection.
    pub collection_id: i64,
    /// Owner-facing label.
    pub label: Option<String>,
    /// Creator account.
    pub created_by: i64,
    /// Creation timestamp (RFC 3339 text).
    pub created_at: String,
    /// Expiry timestamp (RFC 3339 text).
    pub expires_at: Option<String>,
    /// Argon2 password hash, when protected.
    pub password_hash: Option<String>,
    /// Public page views so far.
    pub view_count: i64,
    /// Last public view.
    pub last_viewed_at: Option<String>,
    /// Revocation timestamp (RFC 3339 text).
    pub revoked_at: Option<String>,
    /// Collection name from the join.
    pub collection_name: String,
    /// Collection description from the join.
    pub collection_description: Option<String>,
    /// Collection kind from the join.
    pub collection_kind: String,
    /// Collection visibility from the join.
    pub collection_visibility: String,
    /// Collection owner from the join.
    pub collection_user_id: i64,
    /// Collection last update (RFC 3339 text).
    pub collection_updated_at: String,
}

impl CollectionShareRow {
    /// Converts the row to the owner-facing API shape.
    #[must_use]
    pub fn to_link(&self, token: Option<String>) -> CollectionShareLink {
        CollectionShareLink {
            id: self.id,
            token,
            collection_id: self.collection_id,
            collection_name: self.collection_name.clone(),
            label: self.label.clone(),
            expires_at: self.expires_at.as_deref().and_then(db::parse_time),
            has_password: self.password_hash.is_some(),
            view_count: self.view_count,
            revoked_at: self.revoked_at.as_deref().and_then(db::parse_time),
            created_at: db::parse_time(&self.created_at).unwrap_or_default(),
        }
    }

    /// Parsed collection visibility.
    #[must_use]
    pub fn visibility(&self) -> CollectionVisibility {
        CollectionVisibility::parse(&self.collection_visibility)
            .unwrap_or(CollectionVisibility::Private)
    }

    /// Parsed collection kind.
    #[must_use]
    pub fn kind(&self) -> CollectionKind {
        CollectionKind::parse(&self.collection_kind).unwrap_or(CollectionKind::Manual)
    }
}

/// Values written by `POST /api/collections/{id}/share`.
#[derive(Debug)]
pub struct NewCollectionShare<'a> {
    /// SHA-256 of the plaintext token.
    pub token_hash: &'a str,
    /// Shared collection.
    pub collection_id: i64,
    /// Owner-facing label.
    pub label: Option<&'a str>,
    /// Creator account.
    pub created_by: i64,
    /// Expiry, `None` never.
    pub expires_at: Option<DateTime<Utc>>,
    /// Argon2 password hash, when protected.
    pub password_hash: Option<&'a str>,
}

const SHARE_COLUMNS: &str = "s.id, s.collection_id, s.label, s.created_by, s.created_at, \
s.expires_at, s.password_hash, s.view_count, s.last_viewed_at, s.revoked_at, \
c.name AS collection_name, c.description AS collection_description, \
c.kind AS collection_kind, c.visibility AS collection_visibility, \
c.user_id AS collection_user_id, c.updated_at AS collection_updated_at";

const SHARE_FROM: &str = " FROM collection_shares s \
JOIN collections c ON c.id = s.collection_id";

/// Generates a new collection share token: 32 random bytes as `lsc_<hex>`.
#[must_use]
pub fn generate_token() -> String {
    let mut bytes = [0_u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", hex::encode(bytes))
}

/// Hashes a token for storage and lookup.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Inserts a collection share link and returns its id.
pub async fn insert(pool: &Database, share: &NewCollectionShare<'_>) -> Result<i64, ApiError> {
    db::query(
        "INSERT INTO collection_shares
            (collection_id, token_hash, label, created_by, created_at, expires_at,
             password_hash, view_count, last_viewed_at, revoked_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 0, NULL, NULL)",
    )
    .bind(share.collection_id)
    .bind(share.token_hash)
    .bind(share.label)
    .bind(share.created_by)
    .bind(db::db_time(db::now()))
    .bind(share.expires_at.map(db::db_time))
    .bind(share.password_hash)
    .returning_id(pool)
    .await
    .map_err(ApiError::from)
}

/// Reads one share link by id.
pub async fn get(pool: &Database, id: i64) -> Result<Option<CollectionShareRow>, ApiError> {
    let sql = format!("SELECT {SHARE_COLUMNS}{SHARE_FROM} WHERE s.id = ?");
    db::query_as::<CollectionShareRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)
}

/// Reads a share link by token hash. Revocation and expiry are not applied.
pub async fn find_by_token(
    pool: &Database,
    token_hash: &str,
) -> Result<Option<CollectionShareRow>, ApiError> {
    let sql = format!("SELECT {SHARE_COLUMNS}{SHARE_FROM} WHERE s.token_hash = ?");
    db::query_as::<CollectionShareRow>(&sql)
        .bind(token_hash)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)
}

/// Every link of one collection, newest first.
pub async fn list_for_collection(
    pool: &Database,
    collection_id: i64,
) -> Result<Vec<CollectionShareRow>, ApiError> {
    let sql =
        format!("SELECT {SHARE_COLUMNS}{SHARE_FROM} WHERE s.collection_id = ? ORDER BY s.id DESC");
    bind_query_as(
        db::query_as::<CollectionShareRow>(&sql),
        &[Bind::Int(collection_id)],
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)
}

/// Soft-revokes a share link.
pub async fn revoke(pool: &Database, id: i64) -> Result<(), ApiError> {
    db::query(
        "UPDATE collection_shares SET revoked_at = ?
          WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(db::db_time(db::now()))
    .bind(id)
    .execute(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(())
}

/// Applies the lifecycle rules: revoked, expired, or the collection was made
/// private after the link was created.
pub fn validate(share: &CollectionShareRow, now: DateTime<Utc>) -> Result<(), ShareDenial> {
    if share.revoked_at.is_some() {
        return Err(ShareDenial::Revoked);
    }
    if share.visibility() == CollectionVisibility::Private {
        return Err(ShareDenial::NotFound);
    }
    if let Some(expires_at) = share.expires_at.as_deref().and_then(db::parse_time) {
        if expires_at <= now {
            return Err(ShareDenial::Expired);
        }
    }
    Ok(())
}

/// Checks the presented password. Always spends one Argon2 verification, even
/// when the link has no password or does not exist, so response timing cannot
/// reveal whether a token exists or is protected.
#[must_use]
pub fn verify_access(share: Option<&CollectionShareRow>, password: Option<&str>) -> bool {
    match share {
        Some(share) => match share.password_hash.as_deref() {
            Some(hash) => password.is_some_and(|value| auth::verify_password(value, hash)),
            None => {
                spend_password_check();
                true
            }
        },
        None => {
            spend_password_check();
            false
        }
    }
}

/// Counts one successful public view.
pub async fn record_view(pool: &Database, id: i64) -> Result<i64, ApiError> {
    let now = db::db_time(db::now());
    db::query(
        "UPDATE collection_shares
            SET view_count = view_count + 1, last_viewed_at = ?
          WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(ApiError::from)?;
    let count = db::query_scalar::<i64>("SELECT view_count FROM collection_shares WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(count)
}

/// Row of the public game query.
#[derive(Debug, sqlx::FromRow)]
struct PublicGameRow {
    id: i64,
    title: String,
    sort_title: String,
    cover_path: Option<String>,
    release_date: Option<String>,
    genres_json: String,
    rating: Option<f64>,
    max_players: Option<i64>,
    provider_slug: Option<String>,
    version_count: i64,
    minutes_played: i64,
}

impl PublicGameRow {
    fn to_public(&self) -> SharedCollectionGame {
        SharedCollectionGame {
            id: self.id,
            title: self.title.clone(),
            cover_url: self.cover_path.as_deref().map(media::media_url),
            release_year: self
                .release_date
                .as_deref()
                .and_then(|date| date.get(0..4))
                .and_then(|year| year.parse::<i32>().ok()),
            genres: db::parse_json_list(&self.genres_json),
        }
    }

    fn candidate(&self) -> Candidate {
        Candidate {
            title: self.title.clone(),
            release_date: self.release_date.clone(),
            genres: db::parse_json_list(&self.genres_json),
            rating: self.rating,
            max_players: self.max_players.and_then(|value| u32::try_from(value).ok()),
            // Public evaluation is viewer-independent: playtime is 0 and
            // "installed" means a downloadable version exists.
            minutes_played: self.minutes_played,
            provider_slug: self.provider_slug.clone(),
            version_count: self.version_count,
        }
    }
}

const PUBLIC_GAME_COLUMNS: &str = "g.id, g.title, g.sort_title, g.cover_path, g.release_date, \
g.genres_json, g.rating, g.max_players, g.provider_slug, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0) AS version_count, \
CAST(0 AS BIGINT) AS minutes_played";

/// Loads the games a public collection page shows.
///
/// Manual shelves keep insertion order. Smart shelves are evaluated at view
/// time against viewer-independent facts, so every visitor sees the same list.
pub async fn public_games(
    pool: &Database,
    share: &CollectionShareRow,
) -> Result<(Vec<SharedCollectionGame>, i64), ApiError> {
    if share.kind() == CollectionKind::Smart {
        let rules: SmartRules =
            db::query_scalar::<String>("SELECT rules_json FROM collections WHERE id = ?")
                .bind(share.collection_id)
                .fetch_one(pool)
                .await
                .map(|raw| serde_json::from_str(&raw).unwrap_or_default())
                .map_err(ApiError::from)?;
        let sql = format!("SELECT {PUBLIC_GAME_COLUMNS} FROM games g");
        let rows = db::query_as::<PublicGameRow>(&sql)
            .fetch_all(pool)
            .await
            .map_err(ApiError::from)?;
        let mut matched: Vec<PublicGameRow> = rows
            .into_iter()
            .filter(|row| collections::evaluate(&rules, &row.candidate()))
            .collect();
        matched.sort_by(|a, b| {
            a.sort_title
                .to_lowercase()
                .cmp(&b.sort_title.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        let total = i64::try_from(matched.len()).unwrap_or(i64::MAX);
        return Ok((
            matched.iter().map(PublicGameRow::to_public).collect(),
            total,
        ));
    }

    let sql = format!(
        "SELECT {PUBLIC_GAME_COLUMNS} FROM games g \
         JOIN collection_games cg ON cg.game_id = g.id AND cg.collection_id = ? \
         ORDER BY cg.added_at ASC, g.id ASC"
    );
    let rows = bind_query_as(
        db::query_as::<PublicGameRow>(&sql),
        &[Bind::Int(share.collection_id)],
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    let total = i64::try_from(rows.len()).unwrap_or(i64::MAX);
    Ok((rows.iter().map(PublicGameRow::to_public).collect(), total))
}

/// Builds the public view. `include_games` is false for a password-protected
/// preview: the metadata stays visible but the shelf stays hidden until the
/// password is presented.
pub async fn view(
    state: &crate::state::AppState,
    share: &CollectionShareRow,
    include_games: bool,
) -> Result<CollectionShareView, ApiError> {
    let (games, game_count) = if include_games {
        public_games(&state.pool, share).await?
    } else {
        (Vec::new(), game_count(&state.pool, share).await?)
    };

    #[derive(Debug, sqlx::FromRow)]
    struct OwnerRow {
        public_profile: i64,
        display_name: Option<String>,
        username: String,
    }
    let owner = db::query_as::<OwnerRow>(
        "SELECT public_profile, display_name, username FROM users WHERE id = ?",
    )
    .bind(share.collection_user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .and_then(|row| {
        (row.public_profile != 0).then(|| SharedCollectionOwner {
            display_name: row.display_name.unwrap_or(row.username),
        })
    });

    Ok(CollectionShareView {
        collection: SharedCollection {
            name: share.collection_name.clone(),
            description: share.collection_description.clone(),
            kind: share.kind(),
            game_count,
            owner,
            updated_at: db::parse_time(&share.collection_updated_at).unwrap_or_default(),
        },
        games,
        expires_at: share.expires_at.as_deref().and_then(db::parse_time),
        has_password: share.password_hash.is_some(),
    })
}

async fn game_count(pool: &Database, share: &CollectionShareRow) -> Result<i64, ApiError> {
    if share.kind() == CollectionKind::Smart {
        let (_, total) = public_games(pool, share).await?;
        return Ok(total);
    }
    db::query_scalar::<i64>("SELECT COUNT(*) FROM collection_games WHERE collection_id = ?")
        .bind(share.collection_id)
        .fetch_one(pool)
        .await
        .map_err(ApiError::from)
}

fn spend_password_check() {
    use std::sync::OnceLock;
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY
        .get_or_init(|| auth::hash_password("ludex-collection-share-dummy").unwrap_or_default());
    if !hash.is_empty() {
        let _ = auth::verify_password("ludex-collection-share-dummy", hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        visibility: CollectionVisibility,
        expires_at: Option<DateTime<Utc>>,
        revoked_at: Option<DateTime<Utc>>,
    ) -> CollectionShareRow {
        CollectionShareRow {
            id: 1,
            collection_id: 2,
            label: None,
            created_by: 3,
            created_at: db::db_time(db::now()),
            expires_at: expires_at.map(db::db_time),
            password_hash: None,
            view_count: 0,
            last_viewed_at: None,
            revoked_at: revoked_at.map(db::db_time),
            collection_name: "Weekend".to_string(),
            collection_description: None,
            collection_kind: "manual".to_string(),
            collection_visibility: visibility.as_str().to_string(),
            collection_user_id: 3,
            collection_updated_at: db::db_time(db::now()),
        }
    }

    #[test]
    fn tokens_are_random_hashed_and_prefixed() {
        let first = generate_token();
        let second = generate_token();
        assert!(first.starts_with(TOKEN_PREFIX));
        assert_eq!(first.len(), TOKEN_PREFIX.len() + TOKEN_BYTES * 2);
        assert_ne!(first, second);
        assert_eq!(hash_token(&first).len(), 64);
        assert_ne!(hash_token(&first), hash_token(&second));
    }

    #[test]
    fn private_collections_never_validate_and_expiry_is_enforced() {
        let now = db::now();
        assert_eq!(
            validate(&row(CollectionVisibility::Private, None, None), now),
            Err(ShareDenial::NotFound)
        );
        assert_eq!(
            validate(&row(CollectionVisibility::Unlisted, None, None), now),
            Ok(())
        );

        let expired = row(
            CollectionVisibility::Unlisted,
            Some(now - chrono::Duration::hours(1)),
            None,
        );
        assert_eq!(validate(&expired, now), Err(ShareDenial::Expired));

        let revoked = row(CollectionVisibility::Public, None, Some(now));
        assert_eq!(validate(&revoked, now), Err(ShareDenial::Revoked));
    }

    #[test]
    fn passwords_are_enforced_and_unknown_links_cost_a_check() {
        let mut protected = row(CollectionVisibility::Unlisted, None, None);
        protected.password_hash = Some(auth::hash_password("open sesame").expect("hash"));
        assert!(!verify_access(Some(&protected), None));
        assert!(!verify_access(Some(&protected), Some("wrong")));
        assert!(verify_access(Some(&protected), Some("open sesame")));
        assert!(!verify_access(None, None));
    }
}
