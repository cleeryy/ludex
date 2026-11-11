//! Metadata providers: IGDB (when credentials are configured) with a Steam
//! Store fallback. Provider responses are normalised, artwork is cached in the
//! media directory and only local `/api/media/...` URLs are stored.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use ludex_core::models::{ProviderRef, Trailer};
use ludex_core::naming::normalize_title;
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::db;
use crate::media::download_media;
use crate::state::AppState;

const IGDB_TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const IGDB_GAMES_URL: &str = "https://api.igdb.com/v4/games";
const STEAM_SEARCH_URL: &str = "https://store.steampowered.com/api/storesearch/";
const STEAM_DETAILS_URL: &str = "https://store.steampowered.com/api/appdetails";
const IGDB_FIELDS: &str = "name,summary,first_release_date,cover.url,artworks.url,\
screenshots.url,videos.video_id,videos.name,genres.name,involved_companies.company.name,\
involved_companies.developer,involved_companies.publisher,rating,age_ratings.*,url,\
platforms.id,category,alternative_names.name,multiplayer_modes.offlinemax,\
multiplayer_modes.onlinemax,game_modes.name";

/// Cached Twitch application token.
struct CachedToken {
    value: String,
    expires_at: Instant,
}

/// Minimal IGDB client using the Twitch client-credentials flow.
#[derive(Clone)]
pub struct IgdbClient {
    client_id: String,
    client_secret: String,
    http: Client,
    token: Arc<Mutex<Option<CachedToken>>>,
    token_url: String,
    games_url: String,
}

impl IgdbClient {
    /// Builds a client from IGDB application credentials.
    #[must_use]
    pub fn new(http: Client, client_id: String, client_secret: String) -> Self {
        Self::with_endpoints(
            http,
            client_id,
            client_secret,
            IGDB_TOKEN_URL,
            IGDB_GAMES_URL,
        )
    }

    /// Builds a client against explicit endpoints. Tests point this at a local
    /// fixture server so the metadata pipeline never touches the network.
    #[must_use]
    pub fn with_endpoints(
        http: Client,
        client_id: String,
        client_secret: String,
        token_url: impl Into<String>,
        games_url: impl Into<String>,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            http,
            token: Arc::new(Mutex::new(None)),
            token_url: token_url.into(),
            games_url: games_url.into(),
        }
    }

    async fn access_token(&self) -> Result<String> {
        let mut guard = self.token.lock().await;
        if let Some(cached) = guard.as_ref() {
            if Instant::now() < cached.expires_at {
                return Ok(cached.value.clone());
            }
        }

        let form = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("grant_type", "client_credentials"),
        ];
        let response = self
            .http
            .post(&self.token_url)
            .form(&form)
            .send()
            .await
            .context("request IGDB access token")?;
        if !response.status().is_success() {
            anyhow::bail!("IGDB token endpoint returned {}", response.status());
        }
        let token: TokenResponse = response
            .json()
            .await
            .context("decode IGDB token response")?;
        let ttl = token.expires_in.saturating_sub(300).max(60);
        *guard = Some(CachedToken {
            value: token.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(ttl),
        });
        Ok(token.access_token)
    }

    async fn clear_token(&self) {
        *self.token.lock().await = None;
    }

    /// Searches IGDB for the best PC match.
    pub async fn search(&self, title: &str, year: Option<u16>) -> Result<Option<ProviderMetadata>> {
        let query = sanitise_search_term(title);
        if query.is_empty() {
            return Ok(None);
        }
        let body = format!("search \"{query}\"; fields {IGDB_FIELDS}; limit 10;");
        let games = self.query(&body).await?;
        let best = games
            .iter()
            .filter_map(|game| {
                let score = score_igdb(game, title, year)?;
                Some((score, game))
            })
            .max_by_key(|(score, _)| *score)
            .map(|(_, game)| game);
        Ok(best.map(map_igdb))
    }

    /// Searches IGDB and returns every candidate, unscored. Used by the manual
    /// match dialog, where a weak title match must still be pickable.
    pub async fn candidates(&self, title: &str) -> Result<Vec<ProviderMetadata>> {
        let query = sanitise_search_term(title);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let body = format!("search \"{query}\"; fields {IGDB_FIELDS}; limit 20;");
        let games = self.query(&body).await?;
        Ok(games.iter().map(map_igdb).collect())
    }

    /// Fetches one IGDB record by id. Returns `Ok(None)` when the record no
    /// longer exists (deleted or a search-index artefact).
    pub async fn fetch_by_id(&self, id: &str) -> Result<Option<ProviderMetadata>> {
        let id = id.trim();
        let Some(id) = id.parse::<i64>().ok().filter(|id| *id > 0) else {
            anyhow::bail!("invalid IGDB id: {id}");
        };
        let body = format!("fields {IGDB_FIELDS}; where id = {id};");
        let games = self.query(&body).await?;
        Ok(games.first().map(map_igdb))
    }

    /// Posts one APIcalypse query and decodes the response.
    async fn query(&self, body: &str) -> Result<Vec<IgdbGame>> {
        let mut retried = false;
        loop {
            let token = self.access_token().await?;
            let response = self
                .http
                .post(&self.games_url)
                .header("Client-ID", &self.client_id)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/json")
                .body(body.to_string())
                .send()
                .await
                .context("query IGDB games")?;

            if response.status() == reqwest::StatusCode::UNAUTHORIZED && !retried {
                self.clear_token().await;
                retried = true;
                continue;
            }
            if !response.status().is_success() {
                anyhow::bail!("IGDB games endpoint returned {}", response.status());
            }
            return response.json().await.context("decode IGDB games response");
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct IgdbGame {
    id: i64,
    name: Option<String>,
    summary: Option<String>,
    first_release_date: Option<i64>,
    cover: Option<IgdbImage>,
    #[serde(default)]
    artworks: Vec<IgdbImage>,
    #[serde(default)]
    screenshots: Vec<IgdbImage>,
    #[serde(default)]
    videos: Vec<IgdbVideo>,
    #[serde(default)]
    genres: Vec<IgdbNamed>,
    #[serde(default)]
    involved_companies: Vec<IgdbCompany>,
    rating: Option<f64>,
    #[serde(default)]
    age_ratings: Vec<IgdbAgeRating>,
    url: Option<String>,
    #[serde(default)]
    platforms: Vec<IgdbNamed>,
    category: Option<i64>,
    #[serde(default)]
    alternative_names: Vec<IgdbNamed>,
    #[serde(default)]
    multiplayer_modes: Vec<IgdbMultiplayerMode>,
    #[serde(default)]
    game_modes: Vec<IgdbNamed>,
}

#[derive(Debug, Deserialize)]
struct IgdbMultiplayerMode {
    #[serde(default)]
    offlinemax: Option<i64>,
    #[serde(default)]
    onlinemax: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct IgdbImage {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IgdbVideo {
    video_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IgdbNamed {
    id: Option<i64>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IgdbCompany {
    #[serde(default)]
    company: Option<IgdbNamed>,
    #[serde(default)]
    developer: Option<bool>,
    #[serde(default)]
    publisher: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct IgdbAgeRating {
    #[serde(default)]
    organization: Option<i64>,
    #[serde(default)]
    rating_category: Option<i64>,
    // Older payloads used `category`/`rating`; accepted for robustness.
    #[serde(default)]
    category: Option<i64>,
    #[serde(default)]
    rating: Option<i64>,
}

fn score_igdb(game: &IgdbGame, query: &str, year: Option<u16>) -> Option<i32> {
    let name = game.name.as_deref()?;
    let platform_ids: Vec<i64> = game.platforms.iter().filter_map(|p| p.id).collect();
    let has_pc = platform_ids.contains(&6);
    let web_only = !platform_ids.is_empty() && platform_ids.iter().all(|id| *id == 82);
    let candidate_year = game
        .first_release_date
        .filter(|timestamp| *timestamp > 0)
        .and_then(timestamp_year);
    score_candidate(
        query,
        name,
        &MatchContext {
            query_year: year,
            candidate_year,
            has_pc,
            web_only,
            has_release_date: game.first_release_date.is_some(),
            category: game.category,
        },
    )
}

fn map_igdb(game: &IgdbGame) -> ProviderMetadata {
    let release_date = game
        .first_release_date
        .filter(|timestamp| *timestamp > 0)
        .and_then(timestamp_date);
    let cover_url = game
        .cover
        .as_ref()
        .and_then(|image| image.url.as_deref())
        .map(|url| igdb_image_url(url, "t_cover_big_2x"));
    let screenshots: Vec<String> = game
        .screenshots
        .iter()
        .filter_map(|image| image.url.as_deref())
        .map(|url| igdb_image_url(url, "t_1080p_2x"))
        .take(8)
        .collect();
    let artwork = game
        .artworks
        .iter()
        .filter_map(|image| image.url.as_deref())
        .map(|url| igdb_image_url(url, "t_1080p_2x"))
        .next();
    let background_url = artwork.or_else(|| screenshots.first().cloned());
    let trailers: Vec<Trailer> = game
        .videos
        .iter()
        .filter_map(|video| {
            let video_id = video
                .video_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())?;
            Some(Trailer {
                title: video
                    .name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| "Trailer".to_string()),
                url: format!("https://www.youtube.com/watch?v={video_id}"),
                thumbnail_url: Some(format!(
                    "https://img.youtube.com/vi/{video_id}/hqdefault.jpg"
                )),
                video_id: Some(video_id.to_string()),
                provider: Some("youtube".to_string()),
            })
        })
        .take(5)
        .collect();

    let age_rating = igdb_age_rating(&game.age_ratings);
    let rating = game.rating.map(|value| value.clamp(0.0, 100.0));
    let alt_titles = names(&game.alternative_names)
        .into_iter()
        .filter(|alias| !alias.eq_ignore_ascii_case(game.name.as_deref().unwrap_or_default()))
        .take(12)
        .collect();
    let mut max_players = game
        .multiplayer_modes
        .iter()
        .flat_map(|mode| [mode.offlinemax, mode.onlinemax])
        .flatten()
        .max()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0);
    if max_players.is_none()
        && game
            .game_modes
            .iter()
            .any(|mode| mode.name.as_deref() == Some("Single player"))
    {
        max_players = Some(1);
    }

    ProviderMetadata {
        title: game.name.clone(),
        description: game
            .summary
            .clone()
            .filter(|value| !value.trim().is_empty()),
        release_date,
        cover_url,
        background_url,
        screenshots,
        trailers,
        genres: names(&game.genres),
        developers: involved_names(&game.involved_companies, true),
        publishers: involved_names(&game.involved_companies, false),
        rating,
        age_rating,
        early_access: None,
        alt_titles,
        max_players,
        platforms: names(&game.platforms),
        provider: ProviderRef {
            slug: "igdb".to_string(),
            id: game.id.to_string(),
            url: game.url.clone(),
        },
    }
}

fn names(items: &[IgdbNamed]) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .iter()
        .filter_map(|item| item.name.clone())
        .filter(|name| !name.trim().is_empty())
        .filter(|name| seen.insert(name.to_ascii_lowercase()))
        .collect()
}

fn involved_names(companies: &[IgdbCompany], developer: bool) -> Vec<String> {
    let mut seen = HashSet::new();
    companies
        .iter()
        .filter(|entry| {
            if developer {
                entry.developer == Some(true)
            } else {
                entry.publisher == Some(true)
            }
        })
        .filter_map(|entry| {
            entry
                .company
                .as_ref()
                .and_then(|company| company.name.clone())
        })
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .filter(|name| seen.insert(name.to_ascii_lowercase()))
        .collect()
}

fn pegi_age(category: i64) -> Option<u8> {
    match category {
        1 => Some(3),
        2 => Some(7),
        3 => Some(12),
        4 => Some(16),
        5 => Some(18),
        _ => None,
    }
}

fn esrb_age(category: i64) -> Option<u8> {
    match category {
        3 => Some(3),
        4 => Some(10),
        5 => Some(13),
        6 => Some(18),
        7 => Some(18),
        _ => None,
    }
}

fn igdb_age_rating(ratings: &[IgdbAgeRating]) -> Option<u8> {
    // Current IGDB payloads expose `organization` + `rating_category`.
    // PEGI is preferred, then ESRB, both mapped to the minimum age.
    let pegi = ratings
        .iter()
        .find(|rating| rating.organization == Some(2))
        .and_then(|rating| rating.rating_category)
        .and_then(pegi_age);
    let esrb = ratings
        .iter()
        .find(|rating| rating.organization == Some(1))
        .and_then(|rating| rating.rating_category)
        .and_then(esrb_age);
    // Legacy shape used `category`/`rating`.
    let legacy = ratings
        .iter()
        .find(|rating| rating.category == Some(2))
        .and_then(|rating| rating.rating)
        .and_then(pegi_age)
        .or_else(|| {
            ratings
                .iter()
                .find(|rating| rating.category == Some(1))
                .and_then(|rating| rating.rating)
                .and_then(esrb_age)
        });
    pegi.or(esrb).or(legacy)
}

/// Normalises an IGDB image URL and swaps the thumbnail size.
#[must_use]
pub fn igdb_image_url(raw: &str, size: &str) -> String {
    let with_scheme = if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    with_scheme.replace("t_thumb", size)
}

fn sanitise_search_term(title: &str) -> String {
    title
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect::<String>()
        .trim()
        .chars()
        .take(120)
        .collect()
}

fn timestamp_date(timestamp: i64) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
}

fn timestamp_year(timestamp: i64) -> Option<u16> {
    timestamp_date(timestamp).map(|date| date.year() as u16)
}

/// Provider-independent metadata used to update a game row.
#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    /// Canonical title of the matched record.
    pub title: Option<String>,
    /// Long description.
    pub description: Option<String>,
    /// Release date.
    pub release_date: Option<DateTime<Utc>>,
    /// Remote cover URL (not yet cached).
    pub cover_url: Option<String>,
    /// Remote background URL (not yet cached).
    pub background_url: Option<String>,
    /// Remote screenshot URLs.
    pub screenshots: Vec<String>,
    /// Trailers.
    pub trailers: Vec<Trailer>,
    /// Genre labels.
    pub genres: Vec<String>,
    /// Developer names.
    pub developers: Vec<String>,
    /// Publisher names.
    pub publishers: Vec<String>,
    /// Rating 0-100.
    pub rating: Option<f64>,
    /// Minimum age.
    pub age_rating: Option<u8>,
    /// Early-access flag when the provider knows it.
    pub early_access: Option<bool>,
    /// Alternative titles used by fuzzy search.
    pub alt_titles: Vec<String>,
    /// Highest supported player count, when known.
    pub max_players: Option<u32>,
    /// Platform labels reported by the provider (display only; enrichment
    /// keeps the library's own `kind`/`platform`).
    pub platforms: Vec<String>,
    /// Provider reference.
    pub provider: ProviderRef,
}

/// Looks a game up through the provider chain, first match wins: IGDB (when
/// configured), then enabled community providers by priority, then Steam.
/// Provider failures are logged and never abort the chain.
pub async fn lookup(
    state: &AppState,
    title: &str,
    year: Option<u16>,
    game_id: Option<i64>,
) -> Option<ProviderMetadata> {
    if let Some(igdb) = &state.igdb {
        match igdb.search(title, year).await {
            Ok(Some(metadata)) => return Some(metadata),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(error = %error, "IGDB lookup failed");
            }
        }
    }

    if let Some(metadata) = provider_lookup(state, title, year, game_id).await {
        return Some(metadata);
    }

    match steam_search(&state.http, title).await {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(error = %error, "Steam lookup failed");
            None
        }
    }
}

/// Tries the enabled community providers in priority order. The first
/// confident match wins; every attempt is recorded in `provider_fetches`.
async fn provider_lookup(
    state: &AppState,
    title: &str,
    year: Option<u16>,
    game_id: Option<i64>,
) -> Option<ProviderMetadata> {
    let configs = state.providers.read().await.clone();
    for config in configs {
        match crate::providers::search(&config, title, year).await {
            Ok(candidates) => {
                crate::providers::record_fetch(
                    &state.pool,
                    config.id,
                    game_id,
                    Ok(candidates.len()),
                )
                .await;
                if let Some(candidate) = crate::providers::best_candidate(&candidates, title, year)
                {
                    tracing::info!(
                        provider = %config.name,
                        game_id,
                        title = %candidate.title,
                        "metadata provider match"
                    );
                    return Some(candidate.to_metadata(&config.slug));
                }
            }
            Err(error) => {
                crate::providers::record_fetch(&state.pool, config.id, game_id, Err(&error)).await;
                tracing::warn!(
                    provider = %config.name,
                    error = %error,
                    "metadata provider lookup failed"
                );
            }
        }
    }
    None
}

/// Enriches one game and caches its artwork. Safe to call concurrently for
/// different games; provider failures are logged and leave the row untouched.
///
/// When the row already stores a provider reference the record is fetched by
/// id, so a refresh keeps the match an editor picked instead of re-running the
/// title search (which could return the wrong first result, #135).
pub async fn enrich_game(state: &AppState, game_id: i64) {
    let game = db::query_as::<(
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    )>(
        "SELECT title, release_date, kind, provider_slug, provider_id
         FROM games WHERE id = ?",
    )
    .bind(game_id)
    .fetch_optional(&state.pool)
    .await;

    let Ok(Some((title, release_date, kind, provider_slug, provider_id))) = game else {
        return;
    };
    let year = release_date
        .as_deref()
        .and_then(|value| value.get(0..4))
        .and_then(|value| value.parse::<u16>().ok());

    // ROMs only consult community providers (or their gamelist): the built-in
    // PC providers would attach the wrong edition.
    let stored = match (
        provider_slug.as_deref().filter(|slug| !slug.is_empty()),
        provider_id.as_deref().filter(|id| !id.is_empty()),
    ) {
        (Some(slug), Some(id)) => Some((slug.to_string(), id.to_string())),
        _ => None,
    };
    let mut metadata = None;
    if let Some((slug, id)) = stored {
        if kind != "rom" || community_slug(state, &slug).await {
            metadata = match fetch_by_id(state, &slug, &id, Some(game_id)).await {
                Ok(Some(metadata)) => Some(metadata),
                Ok(None) => {
                    tracing::warn!(
                        game_id,
                        provider = %slug,
                        provider_id = %id,
                        "stored metadata record disappeared; falling back to a title search"
                    );
                    None
                }
                Err(error) => {
                    tracing::warn!(
                        game_id,
                        provider = %slug,
                        provider_id = %id,
                        error = %error,
                        "could not fetch the stored metadata record; falling back to a title search"
                    );
                    None
                }
            };
        }
    }
    let metadata = match metadata {
        Some(metadata) => Some(metadata),
        None if kind == "rom" => provider_lookup(state, &title, year, Some(game_id)).await,
        None => lookup(state, &title, year, Some(game_id)).await,
    };
    let Some(metadata) = metadata else {
        tracing::debug!(game_id, title = %title, "no metadata match");
        return;
    };

    let artwork = cache_artwork(state, &metadata).await;

    let screenshots_json = if artwork.screenshots.is_empty() {
        None
    } else {
        Some(db::dump_json_list(&artwork.screenshots))
    };
    let trailers_json = if metadata.trailers.is_empty() {
        None
    } else {
        serde_json::to_string(&metadata.trailers).ok()
    };
    let genres_json = list_or_none(&metadata.genres);
    let developers_json = list_or_none(&metadata.developers);
    let publishers_json = list_or_none(&metadata.publishers);
    let alt_titles_json = list_or_none(&metadata.alt_titles).unwrap_or_else(|| "[]".to_string());
    let release_date = metadata.release_date.map(db::db_time);
    let now = db::db_time(db::now());

    let result = db::query(
        "UPDATE games SET
            description = COALESCE(?, description),
            release_date = COALESCE(?, release_date),
            cover_path = COALESCE(?, cover_path),
            background_path = COALESCE(?, background_path),
            screenshots_json = COALESCE(?, screenshots_json),
            trailers_json = COALESCE(?, trailers_json),
            genres_json = COALESCE(?, genres_json),
            developers_json = COALESCE(?, developers_json),
            publishers_json = COALESCE(?, publishers_json),
            rating = COALESCE(?, rating),
            age_rating = COALESCE(?, age_rating),
            early_access = COALESCE(?, early_access),
            alt_titles_json = CASE WHEN ? = '[]' THEN alt_titles_json ELSE ? END,
            max_players = COALESCE(?, max_players),
            provider_slug = ?,
            provider_id = ?,
            provider_url = COALESCE(?, provider_url),
            updated_at = ?
         WHERE id = ?",
    )
    .bind(metadata.description)
    .bind(release_date)
    .bind(artwork.cover)
    .bind(artwork.background)
    .bind(screenshots_json)
    .bind(trailers_json)
    .bind(genres_json)
    .bind(developers_json)
    .bind(publishers_json)
    .bind(metadata.rating)
    .bind(metadata.age_rating.map(i64::from))
    .bind(metadata.early_access.map(i64::from))
    .bind(&alt_titles_json)
    .bind(&alt_titles_json)
    .bind(metadata.max_players.map(i64::from))
    .bind(&metadata.provider.slug)
    .bind(&metadata.provider.id)
    .bind(&metadata.provider.url)
    .bind(now)
    .bind(game_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => tracing::info!(
            game_id,
            provider = %metadata.provider.slug,
            "metadata enriched"
        ),
        Err(error) => tracing::warn!(game_id, error = %error, "metadata update failed"),
    }
}

/// Whether `slug` names an enabled community provider.
async fn community_slug(state: &AppState, slug: &str) -> bool {
    state
        .providers
        .read()
        .await
        .iter()
        .any(|config| config.slug == slug)
}

/// Why a provider record could not be fetched by id.
#[derive(Debug)]
pub enum FetchByIdError {
    /// The provider does not know this id any more.
    NotFound,
    /// Any other provider failure (network, timeout, bad response).
    Failed(String),
}

impl std::fmt::Display for FetchByIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("provider record not found"),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

/// Fetches one record by provider id. `Ok(None)` means the provider answered
/// but no longer returns the record; `Err(NotFound)` is the same outcome
/// reported as an HTTP 404 by a community provider.
pub async fn fetch_by_id(
    state: &AppState,
    slug: &str,
    provider_id: &str,
    game_id: Option<i64>,
) -> Result<Option<ProviderMetadata>, FetchByIdError> {
    match slug {
        "igdb" => {
            let Some(igdb) = &state.igdb else {
                return Ok(None);
            };
            igdb.fetch_by_id(provider_id)
                .await
                .map_err(|error| FetchByIdError::Failed(error.to_string()))
        }
        "steam" => {
            let Ok(app_id) = provider_id.parse::<i64>() else {
                return Ok(None);
            };
            steam_details(&state.http, app_id)
                .await
                .map_err(|error| FetchByIdError::Failed(error.to_string()))
        }
        other => {
            let configs = state.providers.read().await.clone();
            let Some(config) = configs.into_iter().find(|config| config.slug == other) else {
                return Ok(None);
            };
            let result = crate::providers::fetch_by_id(&config, provider_id).await;
            let found = result
                .as_ref()
                .map(|candidate| usize::from(candidate.is_some()));
            crate::providers::record_fetch(&state.pool, config.id, game_id, found).await;
            match result {
                Ok(candidate) => Ok(candidate.map(|candidate| candidate.to_metadata(&config.slug))),
                Err(crate::providers::ProviderError::Status(404, _)) => {
                    Err(FetchByIdError::NotFound)
                }
                Err(error) => Err(FetchByIdError::Failed(error.to_string())),
            }
        }
    }
}

/// Artwork cached during one enrichment or match pass.
#[derive(Debug, Default)]
pub(crate) struct CachedArtwork {
    /// Cached cover file name.
    pub cover: Option<String>,
    /// Cached background file name.
    pub background: Option<String>,
    /// Cached screenshot file names.
    pub screenshots: Vec<String>,
}

/// Downloads and caches the artwork referenced by `metadata`. Replaced files
/// stay on disk until the media GC collects them: they are unreferenced once
/// the row is updated.
pub(crate) async fn cache_artwork(state: &AppState, metadata: &ProviderMetadata) -> CachedArtwork {
    let media_dir = state.config.media_dir.as_path();
    let cover = cache_optional(&state.http, media_dir, metadata.cover_url.as_deref()).await;
    let mut screenshot_names = cache_images(&state.http, media_dir, &metadata.screenshots).await;
    screenshot_names.retain(|name| name.is_some());
    let background =
        match cache_optional(&state.http, media_dir, metadata.background_url.as_deref()).await {
            Some(name) => Some(name),
            None => screenshot_names.iter().flatten().next().cloned(),
        };
    CachedArtwork {
        cover,
        background,
        screenshots: screenshot_names.into_iter().flatten().collect(),
    }
}

fn list_or_none(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| db::dump_json_list(values))
}

async fn cache_optional(http: &Client, media_dir: &Path, url: Option<&str>) -> Option<String> {
    let url = url?;
    match download_media(http, media_dir, url).await {
        Ok(name) => Some(name),
        Err(error) => {
            tracing::warn!(error = %error, "failed to cache artwork");
            None
        }
    }
}

async fn cache_images(http: &Client, media_dir: &Path, urls: &[String]) -> Vec<Option<String>> {
    use futures_util::StreamExt;
    futures_util::stream::iter(urls.iter().cloned())
        .map(|url| {
            let http = http.clone();
            let media_dir = media_dir.to_path_buf();
            async move {
                match download_media(&http, &media_dir, &url).await {
                    Ok(name) => Some(name),
                    Err(error) => {
                        tracing::warn!(error = %error, "failed to cache screenshot");
                        None
                    }
                }
            }
        })
        .buffered(4)
        .collect()
        .await
}

// ---------------------------------------------------------------------------
// Steam Store fallback
// ---------------------------------------------------------------------------

pub(crate) async fn steam_search(http: &Client, title: &str) -> Result<Option<ProviderMetadata>> {
    steam_search_at(STEAM_SEARCH_URL, http, title).await
}

/// [`steam_search`] against an explicit endpoint, for tests.
pub(crate) async fn steam_search_at(
    base: &str,
    http: &Client,
    title: &str,
) -> Result<Option<ProviderMetadata>> {
    let hits = steam_search_hits(base, http, title).await?;
    let best = hits
        .iter()
        .filter(|hit| hit.kind.as_deref() != Some("bundle"))
        .filter_map(|hit| {
            let score = score_candidate(
                title,
                &hit.name,
                &MatchContext {
                    query_year: None,
                    candidate_year: None,
                    has_pc: hit.windows,
                    web_only: false,
                    has_release_date: false,
                    category: None,
                },
            )?;
            Some((score, hit))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, hit)| hit);

    let Some(best) = best else {
        return Ok(None);
    };
    steam_details_at(STEAM_DETAILS_URL, http, best.id).await
}

/// One row of the Steam Store search response.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SteamSearchHit {
    /// Steam application id.
    pub(crate) id: i64,
    /// Store name.
    pub(crate) name: String,
    /// `app`, `bundle`, ...
    #[serde(rename = "type", default)]
    pub(crate) kind: Option<String>,
    /// Small capsule used by the match dialog.
    #[serde(default)]
    pub(crate) tiny_image: Option<String>,
    /// Whether the store lists Windows.
    #[serde(default)]
    pub(crate) windows: bool,
}

/// Queries the Steam Store search endpoint and returns every hit.
pub(crate) async fn steam_search_hits(
    base: &str,
    http: &Client,
    title: &str,
) -> Result<Vec<SteamSearchHit>> {
    let response = http
        .get(base)
        .query(&[("term", title), ("l", "english"), ("cc", "US")])
        .send()
        .await
        .context("query Steam store search")?;
    if !response.status().is_success() {
        anyhow::bail!("Steam store search returned {}", response.status());
    }
    let search: SteamSearchResponse = response
        .json()
        .await
        .context("decode Steam search response")?;
    Ok(search
        .items
        .into_iter()
        .map(|item| SteamSearchHit {
            id: item.id,
            name: item.name,
            kind: item.kind,
            tiny_image: item.tiny_image,
            windows: item
                .platforms
                .as_ref()
                .and_then(|platforms| platforms.windows)
                .unwrap_or(false),
        })
        .collect())
}

/// Steam Store search hits for the manual match dialog (#135).
pub(crate) async fn steam_candidates(http: &Client, title: &str) -> Result<Vec<SteamSearchHit>> {
    steam_search_hits(STEAM_SEARCH_URL, http, title).await
}

pub(crate) async fn steam_details(http: &Client, app_id: i64) -> Result<Option<ProviderMetadata>> {
    steam_details_at(STEAM_DETAILS_URL, http, app_id).await
}

/// [`steam_details`] against an explicit endpoint, for tests.
pub(crate) async fn steam_details_at(
    base: &str,
    http: &Client,
    app_id: i64,
) -> Result<Option<ProviderMetadata>> {
    let response = http
        .get(base)
        .query(&[
            ("appids", app_id.to_string()),
            ("l", "english".to_string()),
            ("cc", "US".to_string()),
        ])
        .send()
        .await
        .context("query Steam app details")?;
    if !response.status().is_success() {
        anyhow::bail!("Steam app details returned {}", response.status());
    }
    let mut envelope: HashMap<String, SteamAppEnvelope> =
        response.json().await.context("decode Steam app details")?;
    let Some(envelope) = envelope.remove(&app_id.to_string()) else {
        return Ok(None);
    };
    if !envelope.success {
        return Ok(None);
    }
    let Some(app) = envelope.data else {
        return Ok(None);
    };
    if matches!(
        app.kind.as_deref(),
        Some("dlc" | "demo" | "mod" | "video" | "hardware" | "series" | "episode")
    ) {
        return Ok(None);
    }

    let release_date = app
        .release_date
        .as_ref()
        .and_then(|release| parse_steam_release(&release.date, release.coming_soon));
    let screenshots: Vec<String> = app
        .screenshots
        .iter()
        .filter_map(|shot| shot.path_full.clone())
        .take(8)
        .collect();
    let trailers: Vec<Trailer> = app
        .movies
        .iter()
        .filter_map(|movie| {
            let url = movie
                .mp4
                .as_ref()
                .and_then(SteamVideo::best)
                .or_else(|| movie.webm.as_ref().and_then(SteamVideo::best))?;
            Some(Trailer {
                title: movie
                    .name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| "Trailer".to_string()),
                url,
                thumbnail_url: movie.thumbnail.clone(),
                video_id: None,
                provider: Some("file".to_string()),
            })
        })
        .take(5)
        .collect();
    let description = app
        .about_the_game
        .as_deref()
        .map(strip_html)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            app.short_description
                .as_deref()
                .map(strip_html)
                .filter(|text| !text.is_empty())
        });
    let genres: Vec<String> = app
        .genres
        .iter()
        .map(|genre| genre.description.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    let early_access = app.genres.iter().any(|genre| {
        genre.id.as_deref() == Some("70") || genre.description.eq_ignore_ascii_case("Early Access")
    });
    let required_age = app
        .required_age
        .as_ref()
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        .filter(|age| *age > 0)
        .and_then(|age| u8::try_from(age).ok());
    let platforms = app
        .platforms
        .as_ref()
        .map(|platforms| {
            [
                (platforms.windows, "Windows"),
                (platforms.mac, "macOS"),
                (platforms.linux, "Linux"),
            ]
            .into_iter()
            .filter(|(supported, _)| *supported)
            .map(|(_, label)| label.to_string())
            .collect()
        })
        .unwrap_or_default();

    Ok(Some(ProviderMetadata {
        title: app.name.clone(),
        description,
        release_date,
        cover_url: app.header_image.clone(),
        background_url: app.background.clone(),
        screenshots,
        trailers,
        genres,
        developers: clean_list(app.developers),
        publishers: clean_list(app.publishers),
        rating: None,
        age_rating: required_age,
        early_access: Some(early_access),
        alt_titles: Vec::new(),
        max_players: None,
        platforms,
        provider: ProviderRef {
            slug: "steam".to_string(),
            id: app_id.to_string(),
            url: Some(format!("https://store.steampowered.com/app/{app_id}")),
        },
    }))
}

fn clean_list(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .collect()
}

#[derive(Debug, Deserialize)]
struct SteamSearchResponse {
    #[serde(default)]
    items: Vec<SteamSearchItem>,
}

#[derive(Debug, Deserialize)]
struct SteamSearchItem {
    id: i64,
    name: String,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    tiny_image: Option<String>,
    #[serde(default)]
    platforms: Option<SteamPlatforms>,
}

#[derive(Debug, Deserialize)]
struct SteamPlatforms {
    #[serde(default)]
    windows: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct SteamAppEnvelope {
    success: bool,
    #[serde(default, deserialize_with = "deserialize_steam_data")]
    data: Option<SteamApp>,
}

/// Steam returns `"data": []` for unavailable apps; treat that as no data
/// instead of failing the whole response.
fn deserialize_steam_data<'de, D>(deserializer: D) -> Result<Option<SteamApp>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        Some(value) if value.is_object() => serde_json::from_value(value)
            .map(Some)
            .map_err(serde::de::Error::custom),
        _ => Ok(None),
    }
}

#[derive(Debug, Deserialize)]
struct SteamApp {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    short_description: Option<String>,
    #[serde(default)]
    about_the_game: Option<String>,
    #[serde(default)]
    header_image: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    screenshots: Vec<SteamScreenshot>,
    #[serde(default)]
    movies: Vec<SteamMovie>,
    #[serde(default)]
    genres: Vec<SteamGenre>,
    #[serde(default)]
    developers: Vec<String>,
    #[serde(default)]
    publishers: Vec<String>,
    #[serde(default)]
    release_date: Option<SteamReleaseDate>,
    #[serde(default)]
    required_age: Option<serde_json::Value>,
    #[serde(default)]
    platforms: Option<SteamAppPlatforms>,
}

#[derive(Debug, Default, Deserialize)]
struct SteamAppPlatforms {
    #[serde(default)]
    windows: bool,
    #[serde(default)]
    mac: bool,
    #[serde(default)]
    linux: bool,
}

#[derive(Debug, Deserialize)]
struct SteamScreenshot {
    #[serde(default)]
    path_full: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SteamMovie {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    mp4: Option<SteamVideo>,
    #[serde(default)]
    webm: Option<SteamVideo>,
}

#[derive(Debug, Deserialize)]
struct SteamVideo {
    #[serde(rename = "480", default)]
    sd: Option<String>,
    #[serde(default)]
    max: Option<String>,
}

impl SteamVideo {
    fn best(&self) -> Option<String> {
        self.max
            .clone()
            .or_else(|| self.sd.clone())
            .filter(|url| !url.is_empty())
    }
}

#[derive(Debug, Deserialize)]
struct SteamGenre {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize)]
struct SteamReleaseDate {
    #[serde(default)]
    date: String,
    #[serde(default)]
    coming_soon: bool,
}

/// Parses Steam's human release dates into midnight UTC.
#[must_use]
pub fn parse_steam_release(date: &str, coming_soon: bool) -> Option<DateTime<Utc>> {
    if coming_soon {
        return None;
    }
    let trimmed = date.trim();
    for format in [
        "%d %b, %Y",
        "%b %d, %Y",
        "%d %B %Y",
        "%B %d, %Y",
        "%Y-%m-%d",
    ] {
        if let Ok(parsed) = chrono::NaiveDate::parse_from_str(trimmed, format) {
            return parsed
                .and_hms_opt(0, 0, 0)
                .map(|naive| Utc.from_utc_datetime(&naive));
        }
    }
    let year = trimmed
        .split(|c: char| !c.is_ascii_digit())
        .find(|part| part.len() == 4)
        .and_then(|part| part.parse::<i32>().ok())
        .filter(|year| (1900..=2100).contains(year))?;
    Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single()
}

/// Removes HTML tags and decodes the entities Steam uses in descriptions.
#[must_use]
pub fn strip_html(input: &str) -> String {
    let mut text = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => {
                in_tag = true;
                if !text.ends_with(' ') {
                    text.push(' ');
                }
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let decoded = decode_entities(&text);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(end) = rest.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|dec| dec.parse::<u32>().ok())
                })
                .and_then(char::from_u32),
        };
        match replacement {
            Some(value) => {
                out.push(value);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// Signals used to rank a provider candidate.
#[derive(Debug, Clone, Copy)]
pub struct MatchContext {
    /// Release year parsed from the library file name.
    pub query_year: Option<u16>,
    /// Release year reported by the provider.
    pub candidate_year: Option<u16>,
    /// Candidate lists the PC platform (IGDB id 6 / Steam windows).
    pub has_pc: bool,
    /// Candidate is a web-browser-only game.
    pub web_only: bool,
    /// Candidate has a release date.
    pub has_release_date: bool,
    /// IGDB category enum, when known.
    pub category: Option<i64>,
}

/// Scores a provider candidate. Returns `None` for weak or irrelevant matches.
#[must_use]
pub fn score_candidate(query: &str, candidate: &str, context: &MatchContext) -> Option<i32> {
    let MatchContext {
        query_year,
        candidate_year,
        has_pc,
        web_only,
        has_release_date,
        category,
    } = *context;
    if web_only {
        return None;
    }
    // DLC, expansions, bundles, mods, episodes, seasons, packs and updates are
    // not library titles.
    if matches!(category, Some(1 | 2 | 3 | 5 | 6 | 7 | 13 | 14)) {
        return None;
    }

    let query = normalize_title(query);
    let candidate = normalize_title(candidate);
    if query.is_empty() || candidate.is_empty() {
        return None;
    }
    let mut score = name_score(&query, &candidate)?;
    if score < 60 {
        return None;
    }

    if let (Some(query_year), Some(candidate_year)) = (query_year, candidate_year) {
        if query_year == candidate_year {
            score += 40;
        } else {
            score -= 80;
        }
    }
    if has_pc {
        score += 60;
    }
    if has_release_date {
        score += 10;
    }
    match category {
        Some(0) => score += 10,
        Some(_) => score -= 40,
        None => {}
    }
    Some(score)
}

fn name_score(query: &str, candidate: &str) -> Option<i32> {
    if query == candidate {
        return Some(1000);
    }
    let query_tokens: HashSet<&str> = query.split_whitespace().collect();
    let candidate_tokens: HashSet<&str> = candidate.split_whitespace().collect();
    if query_tokens.is_empty() || candidate_tokens.is_empty() {
        return None;
    }
    let overlap = query_tokens.intersection(&candidate_tokens).count();
    let longest = query_tokens.len().max(candidate_tokens.len());
    Some(i32::try_from(overlap * 100 / longest).unwrap_or(0))
}

/// Lenient ranking used by the manual match list (#135): unlike
/// [`score_candidate`] it never rejects a candidate, because the editor is
/// looking at the list precisely because the automatic pick was wrong.
#[must_use]
pub fn rank_score(
    query: &str,
    candidate: &str,
    query_year: Option<u16>,
    candidate_year: Option<u16>,
    has_pc: bool,
) -> i32 {
    let query = normalize_title(query);
    let candidate = normalize_title(candidate);
    if query.is_empty() || candidate.is_empty() {
        return 0;
    }
    let mut score = name_score(&query, &candidate).unwrap_or(0);
    if let (Some(query_year), Some(candidate_year)) = (query_year, candidate_year) {
        if query_year == candidate_year {
            score += 120;
        } else {
            score -= 60;
        }
    } else if candidate_year.is_some() {
        score += 20;
    }
    if has_pc {
        score += 40;
    }
    score.max(0)
}

/// Maps a [`rank_score`] onto the 0..1 confidence shown by the match dialog.
#[must_use]
pub fn confidence(score: i32) -> f64 {
    (f64::from(score) / 1160.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn score(
        query: &str,
        candidate: &str,
        query_year: Option<u16>,
        candidate_year: Option<u16>,
        has_pc: bool,
        web_only: bool,
        has_release_date: bool,
        category: Option<i64>,
    ) -> Option<i32> {
        score_candidate(
            query,
            candidate,
            &MatchContext {
                query_year,
                candidate_year,
                has_pc,
                web_only,
                has_release_date,
                category,
            },
        )
    }

    #[test]
    fn igdb_urls_get_a_scheme_and_size() {
        let raw = "//images.igdb.com/igdb/image/upload/t_thumb/abc123.jpg";
        assert_eq!(
            igdb_image_url(raw, "t_cover_big_2x"),
            "https://images.igdb.com/igdb/image/upload/t_cover_big_2x/abc123.jpg"
        );
        assert_eq!(
            igdb_image_url("images.igdb.com/x/t_thumb/y.jpg", "t_1080p_2x"),
            "https://images.igdb.com/x/t_1080p_2x/y.jpg"
        );
        assert_eq!(
            igdb_image_url("https://cdn.example/a.jpg", "t_cover_big_2x"),
            "https://cdn.example/a.jpg"
        );
    }

    #[test]
    fn html_is_stripped_and_entities_decoded() {
        let input = "<p>Hello <b>world</b> &amp; friends<br/>Second&nbsp;line &#39;quoted&#39;</p>";
        assert_eq!(
            strip_html(input),
            "Hello world & friends Second line 'quoted'"
        );
        assert_eq!(strip_html("plain text"), "plain text");
        assert_eq!(strip_html(""), "");
        assert_eq!(strip_html("a &unknown; b"), "a &unknown; b");
    }

    #[test]
    fn exact_match_beats_fuzzy_and_pc_beats_console() {
        let exact = score("Hades", "Hades", None, None, true, false, true, Some(0));
        let fuzzy = score("Hades", "Hades II", None, None, true, false, true, Some(0));
        assert!(exact.expect("exact") > 1000);
        assert!(fuzzy.is_none(), "weak match must be rejected");

        let pc = score("Hades", "Hades", None, None, true, false, true, Some(0));
        let console = score("Hades", "Hades", None, None, false, false, true, Some(0));
        assert!(pc.expect("pc") > console.expect("console"));
    }

    #[test]
    fn web_only_and_dlc_are_rejected() {
        assert!(score(
            "It Takes Two",
            "It Takes Two",
            None,
            None,
            false,
            true,
            true,
            None
        )
        .is_none());
        assert!(score("Hades", "Hades", None, None, true, false, true, Some(1)).is_none());
        assert!(score("Hades", "Hades", None, None, true, false, true, Some(0)).is_some());
    }

    #[test]
    fn year_mismatch_penalises_but_exact_title_still_matches() {
        let matching = score(
            "Doom",
            "Doom",
            Some(2016),
            Some(2016),
            true,
            false,
            true,
            Some(0),
        );
        let mismatched = score(
            "Doom",
            "Doom",
            Some(2016),
            Some(1993),
            true,
            false,
            true,
            Some(0),
        );
        assert!(matching.expect("match") > mismatched.expect("mismatch"));
        assert!(score(
            "Doom",
            "Doom",
            Some(2016),
            Some(1993),
            true,
            false,
            true,
            Some(0)
        )
        .is_some());
    }

    #[test]
    fn steam_release_dates_are_parsed() {
        assert_eq!(
            parse_steam_release("12 Nov, 2019", false).map(|date| date.to_string()),
            Some("2019-11-12 00:00:00 UTC".to_string())
        );
        assert_eq!(
            parse_steam_release("2023", false).map(|date| date.to_string()),
            Some("2023-01-01 00:00:00 UTC".to_string())
        );
        assert!(parse_steam_release("Coming soon", true).is_none());
        assert!(parse_steam_release("Q1 2024", false).is_some());
    }

    #[test]
    fn igdb_involved_companies_and_age_ratings_parse() {
        let json = r#"{
            "id": 12345,
            "name": "Cat Quest II",
            "involved_companies": [
                {"company": {"id": 1, "name": "The Gentlebros"}, "developer": true, "publisher": false},
                {"company": {"id": 2, "name": "PQube"}, "developer": false, "publisher": true},
                {"company": {"id": 3, "name": "Ignored"}, "developer": false, "publisher": false}
            ],
            "age_ratings": [{"organization": 2, "rating_category": 2}]
        }"#;
        let game: IgdbGame = serde_json::from_str(json).expect("fixture");
        let metadata = map_igdb(&game);
        assert_eq!(metadata.developers, vec!["The Gentlebros".to_string()]);
        assert_eq!(metadata.publishers, vec!["PQube".to_string()]);
        assert_eq!(metadata.age_rating, Some(7));
        assert_eq!(metadata.provider.slug, "igdb");
    }

    #[test]
    fn legacy_age_rating_shape_is_still_accepted() {
        let json = r#"{"id": 1, "name": "Old", "age_ratings": [{"category": 2, "rating": 4}]}"#;
        let game: IgdbGame = serde_json::from_str(json).expect("fixture");
        assert_eq!(map_igdb(&game).age_rating, Some(16));
    }

    #[test]
    fn igdb_videos_map_to_youtube_trailers() {
        let json = r#"{
            "id": 77,
            "name": "Hades",
            "videos": [
                {"video_id": "dQw4w9WgXcQ", "name": "Launch trailer"},
                {"video_id": "", "name": "Broken"},
                {"name": "No id"}
            ]
        }"#;
        let game: IgdbGame = serde_json::from_str(json).expect("fixture");
        let metadata = map_igdb(&game);
        assert_eq!(metadata.trailers.len(), 1, "{:?}", metadata.trailers);
        let trailer = &metadata.trailers[0];
        assert_eq!(trailer.title, "Launch trailer");
        assert_eq!(trailer.video_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(trailer.provider.as_deref(), Some("youtube"));
        assert_eq!(trailer.url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(
            trailer.thumbnail_url.as_deref(),
            Some("https://img.youtube.com/vi/dQw4w9WgXcQ/hqdefault.jpg")
        );
    }

    #[tokio::test]
    #[ignore = "network test: requires LUDEX_IGDB_CLIENT_ID and LUDEX_IGDB_CLIENT_SECRET"]
    async fn igdb_live_lookup() {
        let Ok(client_id) = std::env::var("LUDEX_IGDB_CLIENT_ID") else {
            eprintln!("LUDEX_IGDB_CLIENT_ID not set; skipping");
            return;
        };
        let Ok(client_secret) = std::env::var("LUDEX_IGDB_CLIENT_SECRET") else {
            eprintln!("LUDEX_IGDB_CLIENT_SECRET not set; skipping");
            return;
        };
        let client = IgdbClient::new(reqwest::Client::new(), client_id, client_secret);
        let metadata = client
            .search("Cat Quest II", Some(2019))
            .await
            .expect("IGDB request")
            .expect("match");
        assert_eq!(metadata.provider.slug, "igdb");
        assert!(metadata.cover_url.is_some());
        assert!(!metadata.developers.is_empty());
    }

    #[tokio::test]
    #[ignore = "network test: Steam Store must be reachable"]
    async fn steam_live_lookup() {
        let http = reqwest::Client::new();
        let metadata = steam_search(&http, "Hades")
            .await
            .expect("Steam request")
            .expect("match");
        assert_eq!(metadata.provider.slug, "steam");
        assert!(metadata.cover_url.is_some());
    }

    #[test]
    fn search_terms_are_sanitised() {
        assert_eq!(
            sanitise_search_term("  Hades \"Ultimate\"  "),
            "Hades Ultimate"
        );
        assert_eq!(sanitise_search_term("a\\b"), "ab");
    }

    #[test]
    fn rank_score_is_lenient_and_rewards_year_and_pc() {
        let exact = rank_score("PEAK", "PEAK", None, Some(2025), true);
        let fuzzy = rank_score("PEAK", "Peak Climber", None, None, false);
        assert!(exact > fuzzy, "{exact} vs {fuzzy}");
        assert_eq!(rank_score("", "PEAK", None, None, false), 0);
        assert!(confidence(exact) > confidence(fuzzy));
        assert!((0.0..=1.0).contains(&confidence(i32::MAX)));
    }

    /// Local HTTP fixture server used by the mocked-provider tests.
    async fn mock_http(handler: fn(&str) -> String) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    loop {
                        let Ok(read) = socket.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            break;
                        }
                        buffer.extend_from_slice(&chunk[..read]);
                        let request = String::from_utf8_lossy(&buffer).to_string();
                        if let Some(end) = request.find("\r\n\r\n") {
                            let length = request[..end]
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if buffer.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    let request = String::from_utf8_lossy(&buffer).to_string();
                    let response = handler(&request);
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{address}"), task)
    }

    fn http_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn igdb_fetches_a_record_by_id() {
        fn handler(request: &str) -> String {
            if request.starts_with("POST /oauth2/token") {
                return http_response(r#"{"access_token":"tok","expires_in":3600}"#);
            }
            assert!(request.contains("client-id: application-id"), "{request}");
            assert!(request.contains("Bearer tok"), "{request}");
            assert!(
                request.contains("fields name,summary") && request.contains("where id = 42;"),
                "{request}"
            );
            http_response(
                r#"[{"id":42,"name":"PEAK","summary":"Climb","first_release_date":1760000000,
                    "platforms":[{"id":6,"name":"PC (Microsoft Windows)"}]}]"#,
            )
        }
        let (base, _task) = mock_http(handler).await;
        let client = IgdbClient::with_endpoints(
            reqwest::Client::new(),
            "application-id".to_string(),
            "secret".to_string(),
            format!("{base}/oauth2/token"),
            format!("{base}/v4/games"),
        );
        let metadata = client
            .fetch_by_id("42")
            .await
            .expect("request")
            .expect("record");
        assert_eq!(metadata.provider.id, "42");
        assert_eq!(metadata.title.as_deref(), Some("PEAK"));
        assert_eq!(
            metadata.platforms,
            vec!["PC (Microsoft Windows)".to_string()]
        );
        assert!(client.fetch_by_id("abc").await.is_err());
        assert!(client.fetch_by_id("0").await.is_err());
    }

    #[tokio::test]
    async fn igdb_candidates_are_unfiltered() {
        fn handler(request: &str) -> String {
            if request.starts_with("POST /oauth2/token") {
                return http_response(r#"{"access_token":"tok","expires_in":3600}"#);
            }
            assert!(request.contains("search \"PEAK\";"), "{request}");
            assert!(request.contains("limit 20;"), "{request}");
            http_response(r#"[{"id":1,"name":"PEAK"},{"id":2,"name":"A Short Hike"}]"#)
        }
        let (base, _task) = mock_http(handler).await;
        let client = IgdbClient::with_endpoints(
            reqwest::Client::new(),
            "application-id".to_string(),
            "secret".to_string(),
            format!("{base}/oauth2/token"),
            format!("{base}/v4/games"),
        );
        let records = client.candidates("PEAK").await.expect("request");
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].title.as_deref(), Some("A Short Hike"));
    }

    #[tokio::test]
    async fn steam_candidates_and_details_are_parsed() {
        fn handler(request: &str) -> String {
            if request.starts_with("GET /storesearch") {
                assert!(request.contains("term=PEAK"), "{request}");
                return http_response(
                    r#"{"items":[
                        {"id":3527290,"name":"PEAK","type":"app","tiny_image":"https://cdn.test/peak.jpg",
                         "platforms":{"windows":true}},
                        {"id":9,"name":"PEAK Soundtrack","type":"music"}
                    ]}"#,
                );
            }
            if request.starts_with("GET /appdetails") {
                assert!(request.contains("appids=3527290"), "{request}");
                return http_response(
                    r#"{"3527290":{"success":true,"data":{
                        "name":"PEAK","type":"game",
                        "about_the_game":"<p>Climb with friends</p>",
                        "header_image":"https://cdn.test/header.jpg",
                        "platforms":{"windows":true,"linux":true},
                        "release_date":{"date":"17 Sep, 2025","coming_soon":false},
                        "developers":["Aggro Crab","Landfall"],
                        "genres":[{"id":"23","description":"Indie"}]
                    }}}"#,
                );
            }
            "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_string()
        }
        let (base, _task) = mock_http(handler).await;
        let http = reqwest::Client::new();
        let hits = steam_search_hits(&format!("{base}/storesearch"), &http, "PEAK")
            .await
            .expect("search");
        assert_eq!(hits.len(), 2);
        assert!(hits[0].windows);
        assert_eq!(
            hits[0].tiny_image.as_deref(),
            Some("https://cdn.test/peak.jpg")
        );

        let metadata = steam_details_at(&format!("{base}/appdetails"), &http, 3_527_290)
            .await
            .expect("details")
            .expect("record");
        assert_eq!(metadata.title.as_deref(), Some("PEAK"));
        assert_eq!(
            metadata.platforms,
            vec!["Windows".to_string(), "Linux".to_string()]
        );
        assert_eq!(metadata.developers.len(), 2);
        assert_eq!(
            metadata
                .release_date
                .map(|date| date.format("%Y").to_string()),
            Some("2025".to_string())
        );
    }
}
