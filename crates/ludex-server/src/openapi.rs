//! Generated OpenAPI 3.1 description of the Ludex HTTP API.
//!
//! The document is built from [`ROUTES`], the same table that the contract
//! test compares against the axum router source. Each entry names the request
//! and response schemas that the handler actually uses; schemas come from
//! `utoipa::ToSchema` derives on the DTOs, so the document cannot drift from
//! the types. `tests/openapi.rs` fails when a registered route is missing from
//! this table, when a path is documented twice, or when a referenced schema is
//! not registered.

use utoipa::openapi::info::InfoBuilder;
use utoipa::openapi::path::{OperationBuilder, Parameter};
use utoipa::openapi::request_body::RequestBodyBuilder;
use utoipa::openapi::response::ResponseBuilder;
use utoipa::openapi::schema::{ArrayBuilder, ComponentsBuilder, ObjectBuilder, Type};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::openapi::{
    Content, HttpMethod, OpenApi, OpenApiBuilder, Paths, Ref, RefOr, Response, Responses, Schema,
    Server, Tag,
};
use utoipa::ToSchema;

/// Authentication required by an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// No credential required.
    Public,
    /// Any authenticated account (JWT or API key).
    Session,
    /// Admin or editor role.
    Editor,
    /// Administrator role.
    Admin,
}

impl Auth {
    fn description(self) -> &'static str {
        match self {
            Auth::Public => "Public: no authentication required.",
            Auth::Session => "Requires `Authorization: Bearer <token>` (session JWT or API key).",
            Auth::Editor => {
                "Requires an admin or editor account with the `write` capability (API keys need the `write` scope)."
            }
            Auth::Admin => {
                "Requires an administrator account (API keys need the `admin` scope)."
            }
        }
    }
}

/// Shape of a successful response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload {
    /// No body (for example `204 No Content`).
    None,
    /// A JSON object described by a registered component.
    Schema(&'static str),
    /// An array of a registered component.
    Array(&'static str),
    /// A free-form JSON object.
    Object,
    /// A binary stream (archive, media, save).
    Binary,
}

/// One documented operation.
#[derive(Clone)]
pub struct RouteDoc {
    /// HTTP method.
    pub method: HttpMethod,
    /// Path relative to `/api`, using axum `{param}` placeholders.
    pub path: &'static str,
    /// OpenAPI tag.
    pub tag: &'static str,
    /// Short summary.
    pub summary: &'static str,
    /// Longer description, including auth requirements.
    pub description: &'static str,
    /// Credential required.
    pub auth: Auth,
    /// JSON request body schema name.
    pub request: Option<&'static str>,
    /// Successful response.
    pub response: Payload,
    /// Success status code.
    pub status: u16,
    /// Query parameters: `(name, description)`.
    pub query: &'static [(&'static str, &'static str)],
}

const fn route(
    method: HttpMethod,
    path: &'static str,
    tag: &'static str,
    summary: &'static str,
    auth: Auth,
    response: Payload,
) -> RouteDoc {
    let status = match &method {
        HttpMethod::Post => 201,
        _ => 200,
    };
    RouteDoc {
        method,
        path,
        tag,
        summary,
        description: "",
        auth,
        request: None,
        response,
        status,
        query: &[],
    }
}

const fn body(mut route: RouteDoc, request: &'static str) -> RouteDoc {
    route.request = Some(request);
    route
}

const fn ok(mut route: RouteDoc, status: u16) -> RouteDoc {
    route.status = status;
    route
}

const fn query(mut route: RouteDoc, params: &'static [(&'static str, &'static str)]) -> RouteDoc {
    route.query = params;
    route
}

/// Every route served under `/api` plus the ingest API.
///
/// The order is documentation order; the contract test treats it as a set.
pub const ROUTES: &[RouteDoc] = &[
    // System
    route(
        HttpMethod::Get,
        "/health",
        "System",
        "Health probe",
        Auth::Public,
        Payload::Schema("HealthResponse"),
    ),
    route(
        HttpMethod::Get,
        "/health/details",
        "System",
        "Detailed health and capacity report",
        Auth::Public,
        Payload::Schema("HealthDetails"),
    ),
    route(
        HttpMethod::Get,
        "/setup/status",
        "System",
        "First-run setup status",
        Auth::Public,
        Payload::Schema("SetupStatus"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/setup",
            "System",
            "Create the first administrator",
            Auth::Public,
            Payload::Schema("AuthResponse"),
        ),
        "SetupRequest",
    ),
    route(
        HttpMethod::Get,
        "/config",
        "System",
        "Public server configuration",
        Auth::Public,
        Payload::Schema("ServerConfig"),
    ),
    // OIDC administration (#103)
    route(
        HttpMethod::Get,
        "/admin/oidc",
        "Authentication",
        "Effective OIDC settings, environment locks and discovery status",
        Auth::Admin,
        Payload::Object,
    ),
    body(
        route(
            HttpMethod::Put,
            "/admin/oidc",
            "Authentication",
            "Validate, encrypt, discover and hot-swap OIDC settings",
            Auth::Admin,
            Payload::Object,
        ),
        "OidcSettingsRequest",
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/admin/oidc/test",
                "Authentication",
                "Run OIDC discovery against submitted values",
                Auth::Admin,
                Payload::Object,
            ),
            200,
        ),
        "OidcSettingsRequest",
    ),
    ok(
        route(
            HttpMethod::Post,
            "/admin/oidc/reset",
            "Authentication",
            "Delete saved OIDC settings and fall back to the environment",
            Auth::Admin,
            Payload::Object,
        ),
        200,
    ),
    route(
        HttpMethod::Get,
        "/openapi.json",
        "System",
        "This OpenAPI document",
        Auth::Public,
        Payload::Object,
    ),
    route(
        HttpMethod::Get,
        "/docs",
        "System",
        "Optional HTML API explorer",
        Auth::Public,
        Payload::Binary,
    ),
    // Authentication
    body(
        route(
            HttpMethod::Post,
            "/auth/login",
            "Authentication",
            "Password login",
            Auth::Public,
            Payload::Schema("AuthResponse"),
        ),
        "LoginRequest",
    ),
    route(
        HttpMethod::Get,
        "/auth/me",
        "Authentication",
        "Current account",
        Auth::Session,
        Payload::Schema("UserResponse"),
    ),
    ok(
        route(
            HttpMethod::Post,
            "/auth/logout",
            "Authentication",
            "Revoke the current token",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/change-password",
                "Authentication",
                "Change the account password",
                Auth::Session,
                Payload::None,
            ),
            204,
        ),
        "ChangePasswordRequest",
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/me/password",
                "Authentication",
                "Change the account password (alias)",
                Auth::Session,
                Payload::None,
            ),
            204,
        ),
        "ChangePasswordRequest",
    ),
    route(
        HttpMethod::Get,
        "/auth/sessions",
        "Authentication",
        "List login sessions",
        Auth::Session,
        Payload::Array("AuthSession"),
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/auth/sessions/{id}",
            "Authentication",
            "Revoke one session",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Get,
        "/auth/oidc/start",
        "Authentication",
        "Start the OIDC authorization flow",
        Auth::Public,
        Payload::Binary,
    ),
    route(
        HttpMethod::Get,
        "/auth/oidc/callback",
        "Authentication",
        "OIDC provider callback",
        Auth::Public,
        Payload::Binary,
    ),
    body(
        route(
            HttpMethod::Post,
            "/auth/oidc/exchange",
            "Authentication",
            "Exchange a one-time SSO code for a token",
            Auth::Public,
            Payload::Schema("AuthResponse"),
        ),
        "OidcExchangeRequest",
    ),
    // Users
    route(
        HttpMethod::Get,
        "/users",
        "Users",
        "List accounts",
        Auth::Admin,
        Payload::Array("User"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/users",
            "Users",
            "Create an account",
            Auth::Admin,
            Payload::Schema("User"),
        ),
        "CreateUserRequest",
    ),
    body(
        route(
            HttpMethod::Patch,
            "/users/{id}",
            "Users",
            "Update an account",
            Auth::Admin,
            Payload::Schema("User"),
        ),
        "UpdateUserRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/users/{id}",
            "Users",
            "Delete an account",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Delete,
        "/users/{id}/sessions",
        "Users",
        "Revoke every session of an account",
        Auth::Admin,
        Payload::Schema("SessionRevocation"),
    ),
    // Games
    query(
        route(
            HttpMethod::Get,
            "/games",
            "Games",
            "List or search the library",
            Auth::Session,
            Payload::Schema("GameList"),
        ),
        &[
            ("q", "Title substring"),
            ("genre", "Exact genre"),
            ("state", "Progress state"),
            ("sort", "Sort column"),
            ("order", "asc or desc"),
            ("page", "1-based page"),
            ("limit", "Page size"),
        ],
    ),
    query(
        route(
            HttpMethod::Get,
            "/games/random",
            "Games",
            "Pick a random game",
            Auth::Session,
            Payload::Schema("GameSummary"),
        ),
        &[
            ("q", "Title substring"),
            ("genre", "Exact genre"),
            ("state", "Progress state"),
        ],
    ),
    route(
        HttpMethod::Get,
        "/games/{id}",
        "Games",
        "Game detail",
        Auth::Session,
        Payload::Schema("GameDetail"),
    ),
    body(
        route(
            HttpMethod::Patch,
            "/games/{id}",
            "Games",
            "Edit game metadata",
            Auth::Editor,
            Payload::Schema("GameDetail"),
        ),
        "UpdateGameRequest",
    ),
    route(
        HttpMethod::Post,
        "/games/{id}/refresh-metadata",
        "Games",
        "Queue a metadata refresh",
        Auth::Editor,
        Payload::Schema("RefreshResponse"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/games/{id}/download-ticket",
            "Games",
            "Issue a short-lived download ticket",
            Auth::Session,
            Payload::Schema("TicketResponse"),
        ),
        "TicketRequest",
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/download",
        "Games",
        "Download the archive (ticket or Bearer)",
        Auth::Public,
        Payload::Binary,
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/latest",
        "Games",
        "Newest version for update checks",
        Auth::Session,
        Payload::Schema("LatestVersion"),
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/{version_id}/stream",
        "Games",
        "Stream the archive as a tar",
        Auth::Public,
        Payload::Binary,
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/{version_id}/local-path",
        "Games",
        "Local install path (desktop local mode)",
        Auth::Session,
        Payload::Schema("LocalPathResponse"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/games/{id}/progress",
            "Games",
            "Set progress state",
            Auth::Session,
            Payload::Schema("ProgressUpdate"),
        ),
        "ProgressRequest",
    ),
    body(
        route(
            HttpMethod::Post,
            "/games/{id}/playtime",
            "Games",
            "Add played minutes",
            Auth::Session,
            Payload::Schema("ProgressUpdate"),
        ),
        "PlaytimeRequest",
    ),
    body(
        route(
            HttpMethod::Put,
            "/games/{id}/bookmark",
            "Games",
            "Bookmark or unbookmark",
            Auth::Session,
            Payload::Schema("BookmarkResponse"),
        ),
        "BookmarkRequest",
    ),
    route(
        HttpMethod::Post,
        "/games/{id}/installed",
        "Games",
        "Report a desktop install",
        Auth::Session,
        Payload::Schema("InstalledReportResponse"),
    ),
    query(
        route(
            HttpMethod::Get,
            "/stats",
            "Games",
            "Library statistics",
            Auth::Session,
            Payload::Schema("Stats"),
        ),
        &[],
    ),
    // Search
    query(
        route(
            HttpMethod::Get,
            "/games/suggest",
            "Search",
            "Fuzzy title suggestions",
            Auth::Session,
            Payload::Schema("SuggestResponse"),
        ),
        &[("q", "Partial title"), ("limit", "Maximum suggestions")],
    ),
    // Library
    route(
        HttpMethod::Get,
        "/library/scan",
        "Library",
        "Scan status",
        Auth::Session,
        Payload::Schema("ScanStatus"),
    ),
    route(
        HttpMethod::Post,
        "/library/scan",
        "Library",
        "Start a library scan",
        Auth::Editor,
        Payload::Schema("ScanResponse"),
    ),
    route(
        HttpMethod::Get,
        "/library/folders",
        "Library",
        "Configured directories",
        Auth::Session,
        Payload::Schema("LibraryFolders"),
    ),
    route(
        HttpMethod::Get,
        "/library/roots",
        "Library",
        "List library roots",
        Auth::Session,
        Payload::Array("LibraryRootView"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/library/roots",
            "Library",
            "Add a library root",
            Auth::Admin,
            Payload::Schema("LibraryRootView"),
        ),
        "CreateLibraryRootRequest",
    ),
    body(
        route(
            HttpMethod::Patch,
            "/library/roots/{id}",
            "Library",
            "Update a library root",
            Auth::Admin,
            Payload::Schema("LibraryRootView"),
        ),
        "UpdateLibraryRootRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/library/roots/{id}",
            "Library",
            "Remove a library root",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Post,
        "/library/roots/{id}/scan",
        "Library",
        "Scan one library root",
        Auth::Editor,
        Payload::Schema("RootScanResponse"),
    ),
    route(
        HttpMethod::Get,
        "/library/watcher",
        "Library",
        "Folder watcher status",
        Auth::Session,
        Payload::Object,
    ),
    // Integrity
    body(
        route(
            HttpMethod::Post,
            "/integrity/check",
            "Integrity",
            "Verify library files",
            Auth::Admin,
            Payload::Schema("IntegrityCheckResponse"),
        ),
        "IntegrityCheckRequest",
    ),
    route(
        HttpMethod::Get,
        "/integrity/reports",
        "Integrity",
        "List integrity reports",
        Auth::Session,
        Payload::Array("IntegrityReportDetail"),
    ),
    route(
        HttpMethod::Get,
        "/integrity/reports/{id}",
        "Integrity",
        "Read one integrity report",
        Auth::Session,
        Payload::Schema("IntegrityReportDetail"),
    ),
    // Media
    query(
        route(
            HttpMethod::Post,
            "/media/gc",
            "Media",
            "Garbage-collect unused media",
            Auth::Admin,
            Payload::Object,
        ),
        &[("graceHours", "Minimum age before deletion")],
    ),
    route(
        HttpMethod::Get,
        "/media/{name}",
        "Media",
        "Cached cover or background",
        Auth::Public,
        Payload::Binary,
    ),
    // News
    route(
        HttpMethod::Get,
        "/news",
        "News",
        "List news posts",
        Auth::Public,
        Payload::Array("NewsPost"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/news",
            "News",
            "Publish a news post",
            Auth::Admin,
            Payload::Schema("NewsPost"),
        ),
        "CreateNewsRequest",
    ),
    body(
        route(
            HttpMethod::Patch,
            "/news/{id}",
            "News",
            "Edit a news post",
            Auth::Admin,
            Payload::Schema("NewsPost"),
        ),
        "UpdateNewsRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/news/{id}",
            "News",
            "Delete a news post",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    // Cloud saves
    route(
        HttpMethod::Get,
        "/games/{id}/saves",
        "Cloud saves",
        "List your save snapshots",
        Auth::Session,
        Payload::Array("SaveSnapshot"),
    ),
    route(
        HttpMethod::Post,
        "/games/{id}/saves",
        "Cloud saves",
        "Upload a save archive (multipart)",
        Auth::Session,
        Payload::Schema("SaveSnapshot"),
    ),
    route(
        HttpMethod::Get,
        "/saves/{id}/download",
        "Cloud saves",
        "Download a save archive",
        Auth::Session,
        Payload::Binary,
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/saves/{id}",
            "Cloud saves",
            "Delete a save snapshot",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    // API keys
    query(
        route(
            HttpMethod::Get,
            "/keys",
            "API keys",
            "List API keys",
            Auth::Session,
            Payload::Array("ApiKey"),
        ),
        &[("all", "Admin: list every account's keys")],
    ),
    body(
        route(
            HttpMethod::Post,
            "/keys",
            "API keys",
            "Create an API key",
            Auth::Session,
            Payload::Schema("CreateApiKeyResponse"),
        ),
        "CreateApiKeyRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/keys/{id}",
            "API keys",
            "Revoke an API key",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    // Audit
    query(
        route(
            HttpMethod::Get,
            "/audit",
            "Audit",
            "Read the audit log",
            Auth::Admin,
            Payload::Schema("AuditPage"),
        ),
        &[("limit", "Page size"), ("before", "Cursor")],
    ),
    // Webhooks
    route(
        HttpMethod::Get,
        "/webhooks",
        "Webhooks",
        "List webhooks",
        Auth::Admin,
        Payload::Array("Webhook"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/webhooks",
            "Webhooks",
            "Create a webhook",
            Auth::Admin,
            Payload::Schema("CreateWebhookResponse"),
        ),
        "CreateWebhookRequest",
    ),
    body(
        route(
            HttpMethod::Patch,
            "/webhooks/{id}",
            "Webhooks",
            "Update a webhook",
            Auth::Admin,
            Payload::Schema("UpdateWebhookResponse"),
        ),
        "UpdateWebhookRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/webhooks/{id}",
            "Webhooks",
            "Delete a webhook",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Post,
        "/webhooks/{id}/test",
        "Webhooks",
        "Send a test delivery",
        Auth::Admin,
        Payload::Schema("WebhookTestResponse"),
    ),
    // Collections
    route(
        HttpMethod::Get,
        "/collections",
        "Collections",
        "List collections",
        Auth::Session,
        Payload::Array("Collection"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/collections",
            "Collections",
            "Create a collection",
            Auth::Session,
            Payload::Schema("Collection"),
        ),
        "CreateCollectionRequest",
    ),
    route(
        HttpMethod::Get,
        "/collections/{id}",
        "Collections",
        "Read a collection",
        Auth::Session,
        Payload::Schema("Collection"),
    ),
    body(
        route(
            HttpMethod::Patch,
            "/collections/{id}",
            "Collections",
            "Update a collection",
            Auth::Session,
            Payload::Schema("Collection"),
        ),
        "UpdateCollectionRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/collections/{id}",
            "Collections",
            "Delete a collection",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    query(
        route(
            HttpMethod::Get,
            "/collections/{id}/games",
            "Collections",
            "List collection games",
            Auth::Session,
            Payload::Schema("CollectionGamesResponse"),
        ),
        &[("page", "Page"), ("limit", "Page size")],
    ),
    body(
        route(
            HttpMethod::Post,
            "/collections/{id}/games",
            "Collections",
            "Add games to a collection",
            Auth::Session,
            Payload::Schema("CollectionGamesResponse"),
        ),
        "CollectionGamesRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/collections/{id}/games/{game_id}",
            "Collections",
            "Remove a game from a collection",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    // Community
    route(
        HttpMethod::Get,
        "/me/profile",
        "Community",
        "Your public profile",
        Auth::Session,
        Payload::Schema("PublicProfile"),
    ),
    body(
        route(
            HttpMethod::Patch,
            "/me/profile",
            "Community",
            "Update your public profile",
            Auth::Session,
            Payload::Schema("PublicProfile"),
        ),
        "UpdateProfileRequest",
    ),
    query(
        route(
            HttpMethod::Get,
            "/community/feed",
            "Community",
            "Public activity feed",
            Auth::Session,
            Payload::Schema("ActivityPage"),
        ),
        &[("limit", "Page size"), ("before", "Cursor")],
    ),
    route(
        HttpMethod::Get,
        "/community/users",
        "Community",
        "Public member directory",
        Auth::Session,
        Payload::Array("PublicProfile"),
    ),
    route(
        HttpMethod::Get,
        "/community/users/{id}",
        "Community",
        "Public profile of one member",
        Auth::Session,
        Payload::Schema("PublicProfileResponse"),
    ),
    query(
        route(
            HttpMethod::Get,
            "/community/users/{id}/activity",
            "Community",
            "Activity of one member",
            Auth::Session,
            Payload::Schema("ActivityPage"),
        ),
        &[("limit", "Page size"), ("before", "Cursor")],
    ),
    // Custom artwork (#38)
    ok(
        route(
            HttpMethod::Post,
            "/games/{id}/artwork",
            "Games",
            "Upload custom cover or background art",
            Auth::Editor,
            Payload::Schema("ArtworkResponse"),
        ),
        201,
    ),
    ok(
        query(
            route(
                HttpMethod::Delete,
                "/games/{id}/artwork",
                "Games",
                "Reset a custom cover or background",
                Auth::Editor,
                Payload::None,
            ),
            &[("kind", "cover or background")],
        ),
        204,
    ),
    // Share links (implemented in `share.rs`, migration 0011)
    body(
        route(
            HttpMethod::Post,
            "/games/{id}/shares",
            "Shares",
            "Create a public share link",
            Auth::Editor,
            Payload::Schema("ShareLink"),
        ),
        "CreateShareRequest",
    ),
    route(
        HttpMethod::Get,
        "/shares",
        "Shares",
        "List share links",
        Auth::Editor,
        Payload::Array("ShareLink"),
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/shares/{id}",
            "Shares",
            "Revoke a share link",
            Auth::Editor,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Get,
        "/share/{token}",
        "Shares",
        "Public metadata for a share link",
        Auth::Public,
        Payload::Schema("ShareView"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/share/{token}/access",
            "Shares",
            "Exchange a share link and password for a download ticket",
            Auth::Public,
            Payload::Schema("ShareAccessResponse"),
        ),
        "ShareAccessRequest",
    ),
    // Resumable uploads
    body(
        route(
            HttpMethod::Post,
            "/uploads",
            "Uploads",
            "Start a resumable archive upload",
            Auth::Editor,
            Payload::Schema("UploadSession"),
        ),
        "CreateUploadRequest",
    ),
    route(
        HttpMethod::Get,
        "/uploads/{id}",
        "Uploads",
        "Upload progress for resume",
        Auth::Editor,
        Payload::Schema("UploadSession"),
    ),
    route(
        HttpMethod::Put,
        "/uploads/{id}/parts/{part}",
        "Uploads",
        "Upload one chunk (idempotent, binary body)",
        Auth::Editor,
        Payload::Schema("UploadPartResponse"),
    ),
    route(
        HttpMethod::Post,
        "/uploads/{id}/complete",
        "Uploads",
        "Verify, assemble and index the archive",
        Auth::Editor,
        Payload::Schema("CompleteUploadResponse"),
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/uploads/{id}",
            "Uploads",
            "Abandon an upload",
            Auth::Editor,
            Payload::None,
        ),
        204,
    ),
    // Ingest (documented paths are relative to /api even though the router
    // mounts them with an absolute prefix).
    route(
        HttpMethod::Get,
        "/ingest/downloads",
        "Ingest",
        "List ingest downloads",
        Auth::Editor,
        Payload::Array("IngestJob"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/ingest/downloads",
            "Ingest",
            "Download an archive from a URL",
            Auth::Editor,
            Payload::Schema("IngestJob"),
        ),
        "CreateDownloadRequest",
    ),
    route(
        HttpMethod::Get,
        "/ingest/downloads/{id}",
        "Ingest",
        "Read one ingest job",
        Auth::Editor,
        Payload::Schema("IngestJob"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/ingest/downloads/{id}",
            "Ingest",
            "Replace a job URL or title",
            Auth::Editor,
            Payload::Schema("IngestJob"),
        ),
        "UpdateDownloadRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/ingest/downloads/{id}",
            "Ingest",
            "Cancel and delete a job",
            Auth::Editor,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Post,
        "/ingest/downloads/{id}/retry",
        "Ingest",
        "Retry a failed job",
        Auth::Editor,
        Payload::Schema("IngestJob"),
    ),
    route(
        HttpMethod::Get,
        "/ingest/files",
        "Ingest",
        "List library archive files",
        Auth::Editor,
        Payload::Array("IngestFile"),
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/ingest/files/{name}",
            "Ingest",
            "Delete a library archive",
            Auth::Editor,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Post,
        "/ingest/files/{name}/verify",
        "Ingest",
        "Verify an archive",
        Auth::Editor,
        Payload::Schema("VerifyResponse"),
    ),
    route(
        HttpMethod::Get,
        "/ingest/stats",
        "Ingest",
        "Ingest statistics",
        Auth::Editor,
        Payload::Schema("IngestStats"),
    ),
    // Achievements
    route(
        HttpMethod::Get,
        "/games/{id}/achievements",
        "Achievements",
        "Achievements of a game for the current account",
        Auth::Session,
        Payload::Schema("GameAchievements"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/games/{id}/achievements",
            "Achievements",
            "Create a manual achievement definition",
            Auth::Admin,
            Payload::Schema("DefinitionResponse"),
        ),
        "DefinitionInput",
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/games/{id}/achievements/scan",
                "Achievements",
                "Scan local achievement providers",
                Auth::Editor,
                Payload::Schema("ScanReport"),
            ),
            "ScanRequest",
        ),
        200,
    ),
    body(
        route(
            HttpMethod::Patch,
            "/games/{id}/achievements/{key}",
            "Achievements",
            "Update a manual achievement definition",
            Auth::Admin,
            Payload::Schema("DefinitionResponse"),
        ),
        "DefinitionPatch",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/games/{id}/achievements/{key}",
            "Achievements",
            "Delete an achievement definition and its unlocks",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/games/{id}/achievements/{key}/unlock",
                "Achievements",
                "Unlock an achievement (idempotent)",
                Auth::Editor,
                Payload::Schema("UnlockResponse"),
            ),
            "UnlockRequest",
        ),
        200,
    ),
    route(
        HttpMethod::Get,
        "/me/achievements",
        "Achievements",
        "Achievements unlocked by the current account",
        Auth::Session,
        Payload::Schema("UserAchievements"),
    ),
    route(
        HttpMethod::Get,
        "/users/{id}/achievements",
        "Achievements",
        "Public achievement showcase",
        Auth::Session,
        Payload::Schema("UserAchievements"),
    ),
];

/// Concrete list shape used by `GET /api/games`.
#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = GameList)]
#[allow(dead_code)]
pub struct GameListDoc {
    items: Vec<ludex_core::models::GameSummary>,
    total: i64,
    page: i64,
    limit: i64,
}

/// `{ "started": bool }` response used by scan triggers.
#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Accepted)]
#[allow(dead_code)]
pub struct AcceptedDoc {
    started: bool,
}

/// `{ "recorded": bool }` response used by the installed report.
#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Recorded)]
#[allow(dead_code)]
pub struct RecordedDoc {
    recorded: bool,
}

/// `{ "revoked": n }` response used by admin session revocation.
#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SessionRevocation)]
#[allow(dead_code)]
pub struct SessionRevocationDoc {
    revoked: u64,
}

/// The contract error envelope.
#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ErrorEnvelope)]
#[allow(dead_code)]
pub struct ErrorEnvelopeDoc {
    error: ErrorBodyDoc,
}

/// Error body.
#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ErrorBody)]
#[allow(dead_code)]
pub struct ErrorBodyDoc {
    code: String,
    message: String,
}

fn json_content(payload: Payload) -> Option<Content> {
    match payload {
        Payload::Schema(name) => Some(Content::new(Some(RefOr::Ref(Ref::from_schema_name(name))))),
        Payload::Array(name) => {
            let items = ArrayBuilder::new()
                .items(RefOr::Ref(Ref::from_schema_name(name)))
                .build();
            Some(Content::new(Some(RefOr::T(Schema::Array(items)))))
        }
        Payload::Object => {
            let object = ObjectBuilder::new().build();
            Some(Content::new(Some(RefOr::T(Schema::Object(object)))))
        }
        Payload::None | Payload::Binary => None,
    }
}

fn response_for(doc: &RouteDoc) -> Response {
    let description = match doc.status {
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        _ => "OK",
    };
    let mut builder = ResponseBuilder::new().description(description);
    if let Some(content) = json_content(doc.response) {
        builder = builder.content("application/json", content);
    } else if doc.response == Payload::Binary {
        builder = builder.content(
            "application/octet-stream",
            Content::new(None::<RefOr<Schema>>),
        );
    }
    builder.build()
}

fn parameters(doc: &RouteDoc) -> Vec<Parameter> {
    let mut parameters = Vec::new();
    for name in path_parameters(doc.path) {
        let mut parameter = Parameter::new(name);
        parameter.schema = Some(RefOr::T(Schema::Object(
            ObjectBuilder::new().schema_type(Type::String).build(),
        )));
        parameters.push(parameter);
    }
    for (name, description) in doc.query {
        let mut parameter = Parameter::new(*name);
        parameter.parameter_in = utoipa::openapi::path::ParameterIn::Query;
        parameter.required = utoipa::openapi::Required::False;
        parameter.description = Some((*description).to_string());
        parameter.schema = Some(RefOr::T(Schema::Object(
            ObjectBuilder::new().schema_type(Type::String).build(),
        )));
        parameters.push(parameter);
    }
    parameters
}

/// Extracts `{name}` placeholders from a path template.
#[must_use]
pub fn path_parameters(path: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            break;
        };
        names.push(&after[..end]);
        rest = &after[end + 1..];
    }
    names
}

fn operation_id(doc: &RouteDoc) -> String {
    let path = doc
        .path
        .trim_matches('/')
        .replace(['{', '}'], "")
        .replace('/', "_");
    let method = match doc.method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        HttpMethod::Options => "options",
        HttpMethod::Head => "head",
        HttpMethod::Trace => "trace",
    };
    format!("{method}_{path}")
}

fn response_with_schema(description: &str, schema: &str) -> Response {
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            Content::new(Some(RefOr::Ref(Ref::from_schema_name(schema)))),
        )
        .build()
}

/// Builds the OpenAPI document.
#[must_use]
pub fn document() -> OpenApi {
    let mut paths = Paths::new();
    for doc in ROUTES {
        let mut operation = OperationBuilder::new()
            .tag(doc.tag)
            .summary(Some(doc.summary))
            .operation_id(Some(operation_id(doc)))
            .description(Some(match doc.description {
                "" => doc.auth.description(),
                description => description,
            }));
        if doc.auth != Auth::Public {
            operation =
                operation.security(SecurityRequirement::new("bearerAuth", Vec::<String>::new()));
        }
        let parameters = parameters(doc);
        if !parameters.is_empty() {
            operation = operation.parameters(Some(parameters));
        }
        if let Some(name) = doc.request {
            let body = RequestBodyBuilder::new()
                .required(Some(utoipa::openapi::Required::True))
                .content(
                    "application/json",
                    Content::new(Some(RefOr::Ref(Ref::from_schema_name(name)))),
                )
                .build();
            operation = operation.request_body(Some(body));
        }
        let mut responses = Responses::new();
        responses
            .responses
            .insert(doc.status.to_string(), RefOr::T(response_for(doc)));
        if doc.auth != Auth::Public {
            responses.responses.insert(
                "401".to_string(),
                RefOr::T(response_with_schema(
                    "Authentication required",
                    "ErrorEnvelope",
                )),
            );
        }
        if matches!(doc.auth, Auth::Editor | Auth::Admin) {
            responses.responses.insert(
                "403".to_string(),
                RefOr::T(response_with_schema(
                    "Insufficient role or scope",
                    "ErrorEnvelope",
                )),
            );
        }
        if matches!(doc.path, "/auth/login") {
            responses.responses.insert(
                "429".to_string(),
                RefOr::T(response_with_schema("Too many attempts", "ErrorEnvelope")),
            );
        }
        operation = operation.responses(responses);
        paths.add_path_operation(doc.path, vec![doc.method.clone()], operation);
    }

    let components = ComponentsBuilder::new()
        .schema_from::<ErrorEnvelopeDoc>()
        .schema_from::<ErrorBodyDoc>()
        .schema_from::<GameListDoc>()
        .schema_from::<AcceptedDoc>()
        .schema_from::<RecordedDoc>()
        .schema_from::<SessionRevocationDoc>()
        .schema_from::<ludex_core::models::Role>()
        .schema_from::<ludex_core::models::ProgressState>()
        .schema_from::<ludex_core::models::User>()
        .schema_from::<ludex_core::models::GameSummary>()
        .schema_from::<ludex_core::models::GameDetail>()
        .schema_from::<ludex_core::models::GameVersion>()
        .schema_from::<ludex_core::models::LatestVersion>()
        .schema_from::<ludex_core::models::LaunchInfo>()
        .schema_from::<ludex_core::models::Trailer>()
        .schema_from::<ludex_core::models::ProviderRef>()
        .schema_from::<ludex_core::models::NewsPost>()
        .schema_from::<ludex_core::models::Stats>()
        .schema_from::<ludex_core::models::ProgressUpdate>()
        .schema_from::<crate::models::LoginRequest>()
        .schema_from::<crate::models::AuthResponse>()
        .schema_from::<crate::models::SetupRequest>()
        .schema_from::<crate::models::SetupStatus>()
        .schema_from::<crate::models::HealthResponse>()
        .schema_from::<crate::models::HealthDetails>()
        .schema_from::<crate::models::HealthDatabase>()
        .schema_from::<crate::models::HealthDisk>()
        .schema_from::<crate::models::HealthLibrary>()
        .schema_from::<crate::models::HealthIngest>()
        .schema_from::<crate::models::HealthWatcher>()
        .schema_from::<crate::metrics::IntegritySummary>()
        .schema_from::<crate::models::ServerConfig>()
        .schema_from::<crate::models::FeatureFlags>()
        .schema_from::<crate::models::ChangePasswordRequest>()
        .schema_from::<crate::models::AuthSession>()
        .schema_from::<crate::models::UserResponse>()
        .schema_from::<crate::models::CreateUserRequest>()
        .schema_from::<crate::models::UpdateUserRequest>()
        .schema_from::<crate::models::UpdateGameRequest>()
        .schema_from::<crate::models::TicketRequest>()
        .schema_from::<crate::models::TicketResponse>()
        .schema_from::<crate::models::ProgressRequest>()
        .schema_from::<crate::models::PlaytimeRequest>()
        .schema_from::<crate::models::BookmarkRequest>()
        .schema_from::<crate::models::BookmarkResponse>()
        .schema_from::<crate::models::RefreshResponse>()
        .schema_from::<crate::models::LocalPathResponse>()
        .schema_from::<crate::models::InstalledReportResponse>()
        .schema_from::<crate::models::ScanStatus>()
        .schema_from::<crate::models::ScanResponse>()
        .schema_from::<crate::models::LibraryFolders>()
        .schema_from::<crate::roots::LibraryRootView>()
        .schema_from::<crate::models::CreateLibraryRootRequest>()
        .schema_from::<crate::models::UpdateLibraryRootRequest>()
        .schema_from::<crate::models::RootScanResponse>()
        .schema_from::<crate::models::IntegrityCheckRequest>()
        .schema_from::<crate::models::IntegrityCheckResponse>()
        .schema_from::<crate::models::IntegrityReportDetail>()
        .schema_from::<crate::integrity::IntegrityReport>()
        .schema_from::<crate::integrity::IntegrityFinding>()
        .schema_from::<crate::models::CreateNewsRequest>()
        .schema_from::<crate::models::UpdateNewsRequest>()
        .schema_from::<crate::models::SaveSnapshot>()
        .schema_from::<crate::models::ApiKey>()
        .schema_from::<crate::models::CreateApiKeyRequest>()
        .schema_from::<crate::models::CreateApiKeyResponse>()
        .schema_from::<crate::models::AuditPage>()
        .schema_from::<crate::models::Webhook>()
        .schema_from::<crate::models::CreateWebhookRequest>()
        .schema_from::<crate::models::CreateWebhookResponse>()
        .schema_from::<crate::models::UpdateWebhookRequest>()
        .schema_from::<crate::models::UpdateWebhookResponse>()
        .schema_from::<crate::models::WebhookTestResponse>()
        .schema_from::<crate::models::Collection>()
        .schema_from::<crate::models::CollectionKind>()
        .schema_from::<crate::models::CreateCollectionRequest>()
        .schema_from::<crate::models::UpdateCollectionRequest>()
        .schema_from::<crate::models::CollectionGamesRequest>()
        .schema_from::<crate::models::CollectionGamesResponse>()
        .schema_from::<crate::models::PublicProfile>()
        .schema_from::<crate::models::PublicProfileResponse>()
        .schema_from::<crate::models::UpdateProfileRequest>()
        .schema_from::<crate::models::ActivityPage>()
        .schema_from::<crate::models::Activity>()
        .schema_from::<crate::models::ActivityUser>()
        .schema_from::<crate::models::ActivityGame>()
        .schema_from::<crate::models::ActivityCollection>()
        .schema_from::<crate::models::CollectionOwner>()
        .schema_from::<crate::models::SmartRules>()
        .schema_from::<crate::models::SmartRule>()
        .schema_from::<crate::models::PublicStats>()
        .schema_from::<crate::models::AuditLog>()
        .schema_from::<crate::models::SuggestResponse>()
        .schema_from::<crate::models::GameSuggestion>()
        .schema_from::<crate::routes::oidc::OidcExchangeRequest>()
        .schema_from::<crate::share::CreateShareRequest>()
        .schema_from::<crate::share::ShareAccessRequest>()
        .schema_from::<crate::share::ShareLink>()
        .schema_from::<crate::share::ShareView>()
        .schema_from::<crate::share::ShareGame>()
        .schema_from::<crate::share::ShareVersion>()
        .schema_from::<crate::share::ShareAccessResponse>()
        .schema_from::<crate::models::CreateUploadRequest>()
        .schema_from::<crate::models::UploadSession>()
        .schema_from::<crate::models::UploadPartResponse>()
        .schema_from::<crate::models::CompleteUploadResponse>()
        .schema_from::<crate::ingest::IngestStatus>()
        .schema_from::<crate::ingest::IngestJob>()
        .schema_from::<crate::ingest::IngestFile>()
        .schema_from::<crate::ingest::IngestStats>()
        .schema_from::<crate::ingest::VerifyResponse>()
        .schema_from::<crate::ingest::types::CreateDownloadRequest>()
        .schema_from::<crate::ingest::types::UpdateDownloadRequest>()
        .schema_from::<crate::achievements::Achievement>()
        .schema_from::<crate::achievements::GameAchievements>()
        .schema_from::<crate::achievements::ScanSummary>()
        .schema_from::<crate::achievements::UserAchievements>()
        .schema_from::<crate::achievements::AchievementUser>()
        .schema_from::<crate::achievements::ScanReport>()
        .schema_from::<crate::achievements::ProviderReport>()
        .schema_from::<crate::achievements::ScanRequest>()
        .schema_from::<crate::achievements::DefinitionInput>()
        .schema_from::<crate::achievements::DefinitionPatch>()
        .schema_from::<crate::achievements::UnlockRequest>()
        .schema_from::<crate::routes::achievements::DefinitionResponse>()
        .schema_from::<crate::routes::achievements::UnlockResponse>()
        .schema_from::<crate::routes::admin_oidc::OidcSettingsRequest>()
        .schema_from::<crate::routes::artwork::ArtworkResponse>()
        .security_scheme(
            "bearerAuth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some(
                        "Session JWT from `POST /api/auth/login`, or a `ldx_` API key.",
                    ))
                    .build(),
            ),
        )
        .build();

    let tags: Vec<Tag> = [
        ("System", "Health, setup and discovery"),
        ("Authentication", "Login, sessions and passwords"),
        ("Users", "Administrator account management"),
        ("Games", "Library browsing, metadata and downloads"),
        ("Search", "Fuzzy title search"),
        ("Library", "Scanning, roots and the folder watcher"),
        ("Integrity", "Archive verification reports"),
        ("Media", "Cover, background and screenshot cache"),
        ("News", "Server news posts"),
        ("Cloud saves", "Per-user save snapshots"),
        ("API keys", "Bearer API keys for integrations"),
        ("Audit", "Persistent audit trail"),
        ("Webhooks", "Outbound HMAC-signed events"),
        ("Collections", "Curated and smart collections"),
        ("Community", "Public profiles and activity"),
        ("Shares", "Public, unauthenticated share links"),
        ("Uploads", "Resumable archive uploads"),
        ("Ingest", "URL download manager"),
        (
            "Achievements",
            "Definitions, unlocks and local provider scans",
        ),
    ]
    .into_iter()
    .map(|(name, description)| {
        let mut tag = Tag::new(name);
        tag.description = Some(description.to_string());
        tag
    })
    .collect();

    OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("Ludex API")
                .version(env!("CARGO_PKG_VERSION"))
                .description(Some(
                    "HTTP API for the Ludex self-hosted game library.\n\n\
                     All paths are relative to `/api`. JSON is `camelCase`. Errors are \
                     `{ \"error\": { \"code\", \"message\" } }`. Authenticated routes accept \
                     `Authorization: Bearer <session JWT>` or `Bearer ldx_<api key>`.\n\n\
                     This document is generated from the server's route table and DTOs; \
                     the contract test fails if the router and the document diverge.",
                ))
                .build(),
        )
        .paths(paths)
        .components(Some(components))
        .servers(Some(vec![Server::new("/api")]))
        .tags(Some(tags))
        .build()
}

/// Every `(method, path)` pair in the document, for contract tests.
#[must_use]
pub fn documented_operations() -> Vec<(String, String)> {
    let mut operations = Vec::new();
    let document = document();
    for (path, item) in &document.paths.paths {
        let present = [
            ("GET", item.get.is_some()),
            ("POST", item.post.is_some()),
            ("PUT", item.put.is_some()),
            ("PATCH", item.patch.is_some()),
            ("DELETE", item.delete.is_some()),
        ];
        for (name, exists) in present {
            if exists {
                operations.push((name.to_string(), path.clone()));
            }
        }
    }
    operations.sort();
    operations
}
