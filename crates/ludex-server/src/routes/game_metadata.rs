//! Manual metadata matching routes (#135).
//!
//! `GET /api/games/{id}/metadata/candidates` searches the configured providers
//! for a title and returns every record, with the current match marked.
//! `POST /api/games/{id}/metadata/apply` fetches one record by id, caches its
//! artwork and writes it to the game row.
//!
//! Both routes are editor/admin only and only run when the dialog opens: an
//! ordinary game page load never talks to a metadata provider.
//!
//! Manual edits are tracked in `games.manual_fields_json` (see
//! `0020_metadata_manual_fields.sql`). Applying a match preserves those fields
//! unless `replaceManual: true`; the provider identity and the cached artwork
//! always follow the selected record.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Datelike;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::metadata::{self, ProviderMetadata};
use crate::providers::MetadataProviderConfig;
use crate::routes::games::fetch_detail;
use crate::sql::Bind;
use crate::state::AppState;

/// Per-provider deadline. The dialog is interactive, so a slow provider is
/// reported as an error state instead of holding the list.
const QUERY_TIMEOUT: Duration = Duration::from_secs(6);
/// Longest accepted search query.
const MAX_QUERY_CHARS: usize = 120;
/// Longest accepted provider id.
const MAX_PROVIDER_ID_CHARS: usize = 128;
/// Steam hits inspected through `appdetails` to fill year/developer.
const MAX_STEAM_DETAILS: usize = 8;

/// Metadata fields a provider match may overwrite.
const MATCH_FIELDS: &[&str] = &[
    "title",
    "releaseDate",
    "description",
    "genres",
    "developers",
    "publishers",
    "rating",
    "ageRating",
    "altTitles",
    "players",
];

/// Routes for the manual match dialog.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/games/{id}/metadata/candidates", get(candidates))
        .route("/games/{id}/metadata/apply", post(apply))
}

/// Query string of `GET /api/games/{id}/metadata/candidates`.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetadataCandidatesQuery {
    /// Search text. Defaults to the game's current title.
    #[serde(default)]
    pub q: Option<String>,
    /// Restrict the search to one provider slug.
    #[serde(default)]
    pub provider: Option<String>,
    /// Only rank candidates from this release year first.
    #[serde(default)]
    pub year: Option<u16>,
}

/// One provider record the editor can pick.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetadataCandidate {
    /// Provider slug (`igdb`, `steam` or a community provider slug).
    pub provider: String,
    /// Human-readable provider name.
    pub provider_name: String,
    /// Provider-side record id, passed back to the apply endpoint.
    pub provider_id: String,
    /// Public provider page.
    pub provider_url: Option<String>,
    /// Record title.
    pub title: String,
    /// Release year, when the provider reports one.
    pub year: Option<u16>,
    /// Cover URL (remote; it is cached when the match is applied).
    pub cover_url: Option<String>,
    /// Long description, HTML stripped.
    pub summary: Option<String>,
    /// Developer names.
    pub developers: Vec<String>,
    /// Platform labels.
    pub platforms: Vec<String>,
    /// Relevance score (0-1160); higher is a closer title/year match.
    pub score: i32,
    /// `score` normalised to 0..1, for the confidence bar.
    pub confidence: f64,
    /// Whether this record is the game's current match.
    pub selected: bool,
}

/// Outcome of one provider query, so a failing provider is visible without
/// turning the whole response into an error.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetadataProviderStatus {
    /// Provider slug.
    pub slug: String,
    /// Human-readable provider name.
    pub name: String,
    /// Whether the query succeeded.
    pub ok: bool,
    /// Safe error message when `ok` is false.
    pub error: Option<String>,
    /// Number of candidates returned.
    pub candidates: usize,
    /// Round-trip duration in milliseconds.
    pub elapsed_ms: u64,
}

/// `GET /api/games/{id}/metadata/candidates` response.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetadataCandidatesResponse {
    /// Query that was searched.
    pub query: String,
    /// Game the list was requested for.
    pub game_id: i64,
    /// Metadata fields edited by hand; `apply` preserves them by default.
    pub manual_fields: Vec<String>,
    /// Candidates, best match first.
    pub candidates: Vec<MetadataCandidate>,
    /// Per-provider outcome, including providers that failed.
    pub providers: Vec<MetadataProviderStatus>,
}

/// `POST /api/games/{id}/metadata/apply` request.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplyMetadataRequest {
    /// Provider slug returned by the candidates endpoint.
    pub provider: String,
    /// Provider-side record id returned by the candidates endpoint.
    pub provider_id: String,
    /// Overwrite fields the editor changed by hand. Defaults to false.
    #[serde(default)]
    pub replace_manual: bool,
}

/// Searches every configured provider for `q`.
async fn candidates(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<MetadataCandidatesQuery>,
) -> Result<Json<MetadataCandidatesResponse>, ApiError> {
    user.require_editor()?;
    let game = db::query_as::<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    )>(
        "SELECT title, release_date, provider_slug, provider_id, manual_fields_json
         FROM games WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;
    let (title, _release_date, provider_slug, provider_id, manual_json) = game;

    let q = normalize_query(query.q.as_deref().unwrap_or(&title))?;
    if let Some(year) = query.year {
        if !(1900..=2200).contains(&year) {
            return Err(ApiError::bad_request(
                "invalid_year",
                "Year must be between 1900 and 2200",
            ));
        }
    }
    let wanted = query
        .provider
        .as_deref()
        .map(validate_provider_slug)
        .transpose()?;
    let current = match (provider_slug, provider_id) {
        (Some(slug), Some(id)) if !slug.is_empty() && !id.is_empty() => Some((slug, id)),
        _ => None,
    };

    let community: Vec<MetadataProviderConfig> = state.providers.read().await.clone();
    if let Some(wanted) = wanted.as_deref() {
        let known = wanted == "igdb"
            || wanted == "steam"
            || community.iter().any(|config| config.slug == wanted);
        if !known {
            return Err(ApiError::bad_request(
                "invalid_provider",
                format!("Unknown metadata provider: {wanted}"),
            ));
        }
        if wanted == "igdb" && state.igdb.is_none() {
            return Err(ApiError::bad_request(
                "invalid_provider",
                "IGDB is not configured on this server",
            ));
        }
    }

    let mut jobs: Vec<Source> = Vec::new();
    if state.igdb.is_some() && wanted.as_deref().is_none_or(|slug| slug == "igdb") {
        jobs.push(Source::Igdb);
    }
    // Steam is the unconfigured fallback: querying it on every dialog open
    // would add a store round-trip even when IGDB or a community provider
    // already answers. An explicit `provider=steam` still forces it.
    let steam_available =
        wanted.as_deref() == Some("steam") || (state.igdb.is_none() && community.is_empty());
    if steam_available && wanted.as_deref().is_none_or(|slug| slug == "steam") {
        jobs.push(Source::Steam);
    }
    for config in community {
        if wanted.as_deref().is_none_or(|slug| slug == config.slug) {
            jobs.push(Source::Community(config));
        }
    }

    let results = join_all(
        jobs.into_iter()
            .map(|source| run_source(&state, source, &q, query.year, id)),
    )
    .await;

    let mut statuses = Vec::with_capacity(results.len());
    let mut raw = Vec::new();
    for (status, mut found) in results {
        statuses.push(status);
        raw.append(&mut found);
    }

    let mut candidates: Vec<MetadataCandidate> = raw
        .into_iter()
        .filter(|candidate| !candidate.title.trim().is_empty())
        .map(|candidate| {
            let score = metadata::rank_score(
                &q,
                &candidate.title,
                query.year,
                candidate.year,
                candidate.has_pc,
            );
            let selected = current.as_ref().is_some_and(|(slug, id)| {
                *slug == candidate.provider && *id == candidate.provider_id
            });
            MetadataCandidate {
                provider: candidate.provider,
                provider_name: candidate.provider_name,
                provider_id: candidate.provider_id,
                provider_url: candidate.provider_url,
                title: candidate.title,
                year: candidate.year,
                cover_url: candidate.cover_url,
                summary: candidate.summary,
                developers: candidate.developers,
                platforms: candidate.platforms,
                score,
                confidence: metadata::confidence(score),
                selected,
            }
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });

    Ok(Json(MetadataCandidatesResponse {
        query: q,
        game_id: id,
        manual_fields: db::parse_json_list(&manual_json),
        candidates,
        providers: statuses,
    }))
}

/// One provider query queued by the candidates handler.
enum Source {
    /// Built-in IGDB client.
    Igdb,
    /// Steam Store search.
    Steam,
    /// Configured community provider.
    Community(MetadataProviderConfig),
}

/// Unranked candidate; the handler scores and sorts the combined list.
struct RawCandidate {
    provider: String,
    provider_name: String,
    provider_id: String,
    provider_url: Option<String>,
    title: String,
    year: Option<u16>,
    cover_url: Option<String>,
    summary: Option<String>,
    developers: Vec<String>,
    platforms: Vec<String>,
    has_pc: bool,
}

impl RawCandidate {
    fn from_metadata(metadata: &ProviderMetadata, provider_name: &str) -> Self {
        let has_pc = metadata.platforms.iter().any(|platform| {
            let platform = platform.to_ascii_lowercase();
            platform == "pc" || platform.starts_with("pc (") || platform.contains("windows")
        });
        Self {
            provider: metadata.provider.slug.clone(),
            provider_name: provider_name.to_string(),
            provider_id: metadata.provider.id.clone(),
            provider_url: metadata.provider.url.clone(),
            title: metadata.title.clone().unwrap_or_default(),
            year: provider_year(metadata),
            cover_url: metadata.cover_url.clone(),
            summary: metadata.description.clone(),
            developers: metadata.developers.clone(),
            platforms: metadata.platforms.clone(),
            has_pc,
        }
    }
}

fn provider_year(metadata: &ProviderMetadata) -> Option<u16> {
    metadata
        .release_date
        .map(|date| u16::try_from(date.year()).unwrap_or(0))
}

/// Runs one provider query, isolated: a failure becomes a status entry.
async fn run_source(
    state: &AppState,
    source: Source,
    q: &str,
    year: Option<u16>,
    game_id: i64,
) -> (MetadataProviderStatus, Vec<RawCandidate>) {
    let started = Instant::now();
    match source {
        Source::Igdb => {
            let Some(igdb) = state.igdb.as_ref() else {
                return (
                    failed_status("igdb", "IGDB", started, "IGDB is not configured"),
                    Vec::new(),
                );
            };
            let result = tokio::time::timeout(QUERY_TIMEOUT, igdb.candidates(q)).await;
            match result {
                Ok(Ok(records)) => {
                    let found: Vec<RawCandidate> = records
                        .iter()
                        .map(|metadata| RawCandidate::from_metadata(metadata, "IGDB"))
                        .collect();
                    (ok_status("igdb", "IGDB", started, found.len()), found)
                }
                Ok(Err(error)) => (
                    failed_status("igdb", "IGDB", started, &error.to_string()),
                    Vec::new(),
                ),
                Err(_) => (
                    failed_status("igdb", "IGDB", started, "request timed out"),
                    Vec::new(),
                ),
            }
        }
        Source::Steam => {
            let result = tokio::time::timeout(QUERY_TIMEOUT, steam_candidates(state, q)).await;
            match result {
                Ok(Ok(found)) => (ok_status("steam", "Steam", started, found.len()), found),
                Ok(Err(error)) => (
                    failed_status("steam", "Steam", started, &error.to_string()),
                    Vec::new(),
                ),
                Err(_) => (
                    failed_status("steam", "Steam", started, "request timed out"),
                    Vec::new(),
                ),
            }
        }
        Source::Community(config) => {
            let name = config.name.clone();
            let result =
                tokio::time::timeout(QUERY_TIMEOUT, crate::providers::search(&config, q, year))
                    .await;
            match result {
                Ok(Ok(items)) => {
                    crate::providers::record_fetch(
                        &state.pool,
                        config.id,
                        Some(game_id),
                        Ok(items.len()),
                    )
                    .await;
                    let found: Vec<RawCandidate> = items
                        .iter()
                        .map(|item| {
                            RawCandidate::from_metadata(
                                &item.to_metadata(&config.slug),
                                &config.name,
                            )
                        })
                        .collect();
                    (ok_status(&config.slug, &name, started, found.len()), found)
                }
                Ok(Err(error)) => {
                    crate::providers::record_fetch(
                        &state.pool,
                        config.id,
                        Some(game_id),
                        Err(&error),
                    )
                    .await;
                    (
                        failed_status(&config.slug, &name, started, &error.to_string()),
                        Vec::new(),
                    )
                }
                Err(_) => {
                    let timeout = crate::providers::ProviderError::Timeout;
                    crate::providers::record_fetch(
                        &state.pool,
                        config.id,
                        Some(game_id),
                        Err(&timeout),
                    )
                    .await;
                    (
                        failed_status(&config.slug, &name, started, "request timed out"),
                        Vec::new(),
                    )
                }
            }
        }
    }
}

fn ok_status(slug: &str, name: &str, started: Instant, count: usize) -> MetadataProviderStatus {
    MetadataProviderStatus {
        slug: slug.to_string(),
        name: name.to_string(),
        ok: true,
        error: None,
        candidates: count,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

fn failed_status(slug: &str, name: &str, started: Instant, error: &str) -> MetadataProviderStatus {
    MetadataProviderStatus {
        slug: slug.to_string(),
        name: name.to_string(),
        ok: false,
        error: Some(error.chars().take(300).collect()),
        candidates: 0,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// Steam Store search, with the top hits expanded through `appdetails` so the
/// list shows a release year and developer.
async fn steam_candidates(state: &AppState, q: &str) -> anyhow::Result<Vec<RawCandidate>> {
    let hits = metadata::steam_candidates(&state.http, q).await?;
    let hits: Vec<_> = hits
        .into_iter()
        .filter(|hit| hit.kind.as_deref() != Some("bundle"))
        .take(MAX_STEAM_DETAILS)
        .collect();
    let details = join_all(hits.iter().map(|hit| async {
        tokio::time::timeout(QUERY_TIMEOUT, metadata::steam_details(&state.http, hit.id))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
    }))
    .await;

    Ok(hits
        .iter()
        .zip(details)
        .map(|(hit, details)| match details {
            Some(metadata) => RawCandidate::from_metadata(&metadata, "Steam"),
            None => RawCandidate {
                provider: "steam".to_string(),
                provider_name: "Steam".to_string(),
                provider_id: hit.id.to_string(),
                provider_url: Some(format!("https://store.steampowered.com/app/{}", hit.id)),
                title: hit.name.clone(),
                year: None,
                cover_url: hit.tiny_image.clone(),
                summary: None,
                developers: Vec::new(),
                platforms: if hit.windows {
                    vec!["Windows".to_string()]
                } else {
                    Vec::new()
                },
                has_pc: hit.windows,
            },
        })
        .collect())
}

/// Applies one provider record to a game.
async fn apply(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<ApplyMetadataRequest>,
) -> Result<Json<ludex_core::models::GameDetail>, ApiError> {
    user.require_editor()?;
    let slug = validate_provider_slug(&request.provider)?;
    let provider_id = validate_provider_id(&slug, &request.provider_id)?;

    let game = db::query_as::<(String, Option<String>, Option<String>, String)>(
        "SELECT title, provider_slug, provider_id, manual_fields_json FROM games WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;
    let (old_title, old_slug, old_id, manual_json) = game;

    if !provider_configured(&state, &slug).await {
        return Err(ApiError::bad_request(
            "invalid_provider",
            format!("Unknown metadata provider: {slug}"),
        ));
    }

    let not_found = || {
        ApiError::not_found(
            "metadata_candidate_not_found",
            "The provider no longer returns this record",
        )
    };
    let metadata = match metadata::fetch_by_id(&state, &slug, &provider_id, Some(id)).await {
        Ok(Some(metadata)) => metadata,
        Ok(None) | Err(metadata::FetchByIdError::NotFound) => return Err(not_found()),
        Err(metadata::FetchByIdError::Failed(message)) => {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "provider_request_failed",
                message,
            ));
        }
    };

    // Artwork is cached before the row is written; a failed download keeps the
    // current file (the `COALESCE` below).
    let artwork = metadata::cache_artwork(&state, &metadata).await;

    let marked: HashSet<String> = db::parse_json_list(&manual_json).into_iter().collect();
    let preserved: Vec<String> = if request.replace_manual {
        Vec::new()
    } else {
        MATCH_FIELDS
            .iter()
            .filter(|field| marked.contains(**field))
            .map(|field| (*field).to_string())
            .collect()
    };
    let keep = |field: &str| preserved.iter().any(|kept| kept == field);

    let mut sets: Vec<&str> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();

    if !keep("title") {
        if let Some(title) = metadata
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            sets.push("title = ?");
            binds.push(Bind::Text(title.to_string()));
            sets.push("sort_title = ?");
            binds.push(Bind::Text(sort_title_for(title)));
        }
    }
    if !keep("description") {
        sets.push("description = COALESCE(?, description)");
        binds.push(Bind::OptText(clean_text(metadata.description.as_deref())));
    }
    if !keep("releaseDate") {
        sets.push("release_date = COALESCE(?, release_date)");
        binds.push(Bind::OptText(metadata.release_date.map(db::db_time)));
    }
    if !keep("genres") {
        sets.push("genres_json = COALESCE(?, genres_json)");
        binds.push(Bind::OptText(list_or_none(&metadata.genres)));
    }
    if !keep("developers") {
        sets.push("developers_json = COALESCE(?, developers_json)");
        binds.push(Bind::OptText(list_or_none(&metadata.developers)));
    }
    if !keep("publishers") {
        sets.push("publishers_json = COALESCE(?, publishers_json)");
        binds.push(Bind::OptText(list_or_none(&metadata.publishers)));
    }
    if !keep("rating") {
        sets.push("rating = COALESCE(?, rating)");
        binds.push(Bind::OptFloat(
            metadata.rating.map(|value| value.clamp(0.0, 100.0)),
        ));
    }
    if !keep("ageRating") {
        sets.push("age_rating = COALESCE(?, age_rating)");
        binds.push(Bind::OptInt(metadata.age_rating.map(i64::from)));
    }
    if !keep("altTitles") {
        sets.push("alt_titles_json = COALESCE(?, alt_titles_json)");
        binds.push(Bind::OptText(list_or_none(&metadata.alt_titles)));
    }
    if !keep("players") {
        sets.push("max_players = COALESCE(?, max_players)");
        binds.push(Bind::OptInt(metadata.max_players.map(i64::from)));
    }

    // Artwork and provider identity always follow the selected record.
    sets.push("cover_path = COALESCE(?, cover_path)");
    binds.push(Bind::OptText(artwork.cover));
    sets.push("background_path = COALESCE(?, background_path)");
    binds.push(Bind::OptText(artwork.background));
    sets.push("screenshots_json = COALESCE(?, screenshots_json)");
    binds.push(Bind::OptText(list_or_none(&artwork.screenshots)));
    sets.push("trailers_json = COALESCE(?, trailers_json)");
    binds.push(Bind::OptText((!metadata.trailers.is_empty()).then(|| {
        serde_json::to_string(&metadata.trailers).unwrap_or_else(|_| "[]".to_string())
    })));
    sets.push("provider_slug = ?");
    binds.push(Bind::Text(metadata.provider.slug.clone()));
    sets.push("provider_id = ?");
    binds.push(Bind::Text(metadata.provider.id.clone()));
    sets.push("provider_url = ?");
    binds.push(Bind::OptText(metadata.provider.url.clone()));
    sets.push("updated_at = ?");
    binds.push(Bind::Text(db::db_time(db::now())));

    binds.push(Bind::Int(id));
    let sql = format!("UPDATE games SET {} WHERE id = ?", sets.join(", "));
    crate::sql::bind_query(db::query(&sql), &binds)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;

    if request.replace_manual {
        let remaining: Vec<String> = db::parse_json_list(&manual_json)
            .into_iter()
            .filter(|field| !MATCH_FIELDS.contains(&field.as_str()))
            .collect();
        db::query("UPDATE games SET manual_fields_json = ? WHERE id = ?")
            .bind(db::dump_json_list(&remaining))
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    let old_provider = match (old_slug, old_id) {
        (Some(slug), Some(provider_id)) if !slug.is_empty() && !provider_id.is_empty() => {
            serde_json::json!({ "slug": slug, "id": provider_id })
        }
        _ => serde_json::Value::Null,
    };
    state.events.emit(
        Event::new(actions::GAME_METADATA_MATCHED, user.actor())
            .target("game", id.to_string())
            .metadata(serde_json::json!({
                "oldProvider": old_provider,
                "newProvider": {
                    "slug": metadata.provider.slug,
                    "id": metadata.provider.id,
                },
                "oldTitle": old_title,
                "newTitle": metadata.title,
                "replaceManual": request.replace_manual,
                "preservedFields": preserved,
            })),
    );

    Ok(Json(fetch_detail(&state, user.id, id).await?))
}

/// Whether `slug` names a provider this server can query right now.
async fn provider_configured(state: &AppState, slug: &str) -> bool {
    match slug {
        "igdb" => state.igdb.is_some(),
        "steam" => true,
        other => state
            .providers
            .read()
            .await
            .iter()
            .any(|config| config.slug == other),
    }
}

/// Trims and bounds the search text.
fn normalize_query(raw: &str) -> Result<String, ApiError> {
    let query: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect();
    if query.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_query",
            "The search query is required",
        ));
    }
    Ok(query)
}

/// Normalises and validates a provider slug.
fn validate_provider_slug(raw: &str) -> Result<String, ApiError> {
    let slug = raw.trim().to_ascii_lowercase();
    if slug.is_empty()
        || slug.len() > 64
        || !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(ApiError::bad_request(
            "invalid_provider",
            "The provider slug is invalid",
        ));
    }
    Ok(slug)
}

/// Normalises and validates a provider record id. Built-in providers use
/// numeric ids; a community provider may use any short string.
fn validate_provider_id(provider: &str, raw: &str) -> Result<String, ApiError> {
    let id = raw.trim();
    if id.is_empty()
        || id.chars().count() > MAX_PROVIDER_ID_CHARS
        || id.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "invalid_provider_id",
            "The provider id is invalid",
        ));
    }
    if matches!(provider, "igdb" | "steam") && !id.parse::<u64>().is_ok_and(|value| value > 0) {
        return Err(ApiError::bad_request(
            "invalid_provider_id",
            "This provider uses numeric ids",
        ));
    }
    Ok(id.to_string())
}

fn clean_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn list_or_none(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| db::dump_json_list(values))
}

/// Mirrors the scanner's sort title derivation.
fn sort_title_for(title: &str) -> String {
    let derived = ludex_core::naming::parse_filename(&format!("{title}.zip")).sort_title;
    if derived.trim().is_empty() {
        title.to_string()
    } else {
        derived
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_trimmed_and_bounded() {
        assert_eq!(normalize_query("  Hades  ").expect("query"), "Hades");
        assert!(normalize_query("   ").is_err());
        let long = "x".repeat(500);
        assert_eq!(
            normalize_query(&long).expect("query").chars().count(),
            MAX_QUERY_CHARS
        );
    }

    #[test]
    fn provider_slugs_and_ids_are_validated() {
        assert_eq!(validate_provider_slug(" Steam ").expect("slug"), "steam");
        assert!(validate_provider_slug("").is_err());
        assert!(validate_provider_slug("bad slug").is_err());
        assert!(validate_provider_slug("x".repeat(65).as_str()).is_err());
        assert_eq!(validate_provider_id("igdb", " 42 ").expect("id"), "42");
        assert!(validate_provider_id("igdb", "abc").is_err());
        assert!(validate_provider_id("igdb", "0").is_err());
        assert!(validate_provider_id("fixture", "").is_err());
        assert_eq!(
            validate_provider_id("fixture", "celeste-1").expect("id"),
            "celeste-1"
        );
    }

    #[test]
    fn sort_titles_follow_the_scanner() {
        assert_eq!(sort_title_for("Hades"), "Hades");
        assert_eq!(sort_title_for("The Witcher 3"), "Witcher 3");
    }
}
