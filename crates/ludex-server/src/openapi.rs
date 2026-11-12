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
    query(
        route(
            HttpMethod::Get,
            "/desktop/download",
            "System",
            "Latest desktop app installers and checksums",
            Auth::Public,
            Payload::Schema("DesktopDownload"),
        ),
        &[(
            "platform",
            "windows, linux or macos; omit to list every asset",
        )],
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
    // Community metadata providers (#28)
    route(
        HttpMethod::Get,
        "/admin/metadata-providers",
        "Metadata",
        "List metadata providers, ordered by priority",
        Auth::Admin,
        Payload::Array("MetadataProviderView"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/admin/metadata-providers",
            "Metadata",
            "Create a metadata provider (token is write-only)",
            Auth::Admin,
            Payload::Schema("MetadataProviderView"),
        ),
        "CreateMetadataProviderRequest",
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/admin/metadata-providers/reorder",
                "Metadata",
                "Rewrite provider priorities from an explicit id order",
                Auth::Admin,
                Payload::Array("MetadataProviderView"),
            ),
            200,
        ),
        "ReorderMetadataProvidersRequest",
    ),
    body(
        route(
            HttpMethod::Put,
            "/admin/metadata-providers/{id}",
            "Metadata",
            "Update a metadata provider (blank token keeps the stored one)",
            Auth::Admin,
            Payload::Schema("MetadataProviderView"),
        ),
        "UpdateMetadataProviderRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/admin/metadata-providers/{id}",
            "Metadata",
            "Delete a metadata provider",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/admin/metadata-providers/{id}/test",
                "Metadata",
                "Query a provider and return its candidates or error",
                Auth::Admin,
                Payload::Schema("ProviderTestResponse"),
            ),
            200,
        ),
        "TestMetadataProviderRequest",
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
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/login/mfa",
                "Authentication",
                "Finish a login with a TOTP or recovery code (#166)",
                Auth::Public,
                Payload::Schema("AuthResponse"),
            ),
            200,
        ),
        "MfaLoginRequest",
    ),
    route(
        HttpMethod::Get,
        "/auth/me",
        "Authentication",
        "Current account",
        Auth::Session,
        Payload::Schema("UserResponse"),
    ),
    // Account avatar (#183)
    route(
        HttpMethod::Post,
        "/account/avatar",
        "Authentication",
        "Upload an avatar (multipart field `file`, PNG/JPEG/WebP, 2 MiB, 4096×4096)",
        Auth::Session,
        Payload::Schema("User"),
    ),
    route(
        HttpMethod::Delete,
        "/account/avatar",
        "Authentication",
        "Remove the avatar and delete the managed file",
        Auth::Session,
        Payload::Schema("User"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/auth/onboarding",
            "Authentication",
            "Update the onboarding preferences of the current account",
            Auth::Session,
            Payload::Schema("UserResponse"),
        ),
        "OnboardingRequest",
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
    route(
        HttpMethod::Delete,
        "/auth/sessions",
        "Authentication",
        "Revoke every other session of the account (#166)",
        Auth::Session,
        Payload::Schema("SessionRevocation"),
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
        "/auth/mfa",
        "Authentication",
        "Second-factor status of the account (#166)",
        Auth::Session,
        Payload::Schema("MfaStatus"),
    ),
    ok(
        route(
            HttpMethod::Post,
            "/auth/mfa/totp/enroll",
            "Authentication",
            "Start a TOTP enrolment; returns the secret once (#166)",
            Auth::Session,
            Payload::Schema("TotpEnrollment"),
        ),
        200,
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/mfa/totp/activate",
                "Authentication",
                "Confirm a TOTP enrolment and receive recovery codes (#166)",
                Auth::Session,
                Payload::Schema("TotpActivated"),
            ),
            200,
        ),
        "TotpActivationRequest",
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/mfa/disable",
                "Authentication",
                "Disable two-factor authentication after re-checking the password (#166)",
                Auth::Session,
                Payload::Schema("MfaStatus"),
            ),
            200,
        ),
        "MfaDisableRequest",
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/mfa/recovery-codes",
                "Authentication",
                "Replace the recovery codes (#166)",
                Auth::Session,
                Payload::Schema("RecoveryCodes"),
            ),
            200,
        ),
        "RecoveryCodesRequest",
    ),
    // Invitations (#166)
    route(
        HttpMethod::Get,
        "/invitations",
        "Invitations",
        "List invitation links",
        Auth::Admin,
        Payload::Array("Invitation"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/invitations",
            "Invitations",
            "Create an expiring invitation link; the token is returned once",
            Auth::Admin,
            Payload::Schema("CreatedInvitation"),
        ),
        "CreateInvitationRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/invitations/{id}",
            "Invitations",
            "Revoke an invitation link",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Get,
        "/invitations/{id}",
        "Invitations",
        "Preview an invitation before registering (the path segment is the token)",
        Auth::Public,
        Payload::Schema("InvitationPreview"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/invitations/{id}/accept",
            "Invitations",
            "Redeem an invitation and sign the new account in",
            Auth::Public,
            Payload::Schema("AuthResponse"),
        ),
        "AcceptInvitationRequest",
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
    // Desktop SSO handoff (#157)
    ok(
        body(
            route(
                HttpMethod::Post,
                "/auth/desktop-handoff",
                "Authentication",
                "Bind the current web session to a desktop sign-in state",
                Auth::Session,
                Payload::Schema("DesktopHandoffResponse"),
            ),
            "DesktopHandoffRequest",
        ),
        200,
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/auth/desktop-handoff/claim",
                "Authentication",
                "Claim a pending desktop handoff exactly once",
                Auth::Public,
                Payload::Schema("DesktopHandoffClaimResponse"),
            ),
            "DesktopHandoffClaimRequest",
        ),
        200,
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
    body(
        ok(
            route(
                HttpMethod::Post,
                "/users/{id}/reset-password",
                "Users",
                "Reset an account password and sign every device out (#166)",
                Auth::Admin,
                Payload::Schema("ResetPasswordResult"),
            ),
            200,
        ),
        "ResetPasswordRequest",
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
    query(
        route(
            HttpMethod::Get,
            "/games/{id}/metadata/candidates",
            "Games",
            "Search metadata providers for a manual match",
            Auth::Editor,
            Payload::Schema("MetadataCandidatesResponse"),
        ),
        &[
            ("q", "Search text; defaults to the game title"),
            ("provider", "Restrict to one provider slug"),
            ("year", "Preferred release year"),
        ],
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/games/{id}/metadata/apply",
                "Games",
                "Apply one provider record to a game",
                Auth::Editor,
                Payload::Schema("GameDetail"),
            ),
            "ApplyMetadataRequest",
        ),
        200,
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
        "/games/{id}/versions",
        "Games",
        "List every version with its archive state (editor)",
        Auth::Editor,
        Payload::Schema("GameVersionsResponse"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/games/{id}/versions/default",
            "Games",
            "Pin the version used by downloads and updates (editor)",
            Auth::Editor,
            Payload::Schema("GameVersionsResponse"),
        ),
        "SetDefaultVersionRequest",
    ),
    query(
        route(
            HttpMethod::Delete,
            "/games/{id}/versions/{version_id}",
            "Games",
            "Delete one archive while keeping the game (editor)",
            Auth::Editor,
            Payload::Schema("DeleteVersionResponse"),
        ),
        &[(
            "confirm",
            "Must be true; the game, its metadata and its saves are kept",
        )],
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/latest",
        "Games",
        "Preferred version for update checks",
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
        "/games/{id}/versions/{version_id}/stream/manifest",
        "Games",
        "Ordered ZIP manifest for resumable stream installs",
        Auth::Public,
        Payload::Schema("StreamManifest"),
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/{version_id}/stream/entries/{index}",
        "Games",
        "Raw content of one archive entry (single Range supported)",
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
    // Library health (#165)
    query(
        route(
            HttpMethod::Get,
            "/admin/health/library",
            "Library health",
            "Read the cached library health report",
            Auth::Editor,
            Payload::Schema("LibraryHealthResponse"),
        ),
        &[
            ("severity", "Filter by error, warning or info"),
            ("category", "Filter by finding category"),
            ("limit", "Maximum findings (1-500, default 200)"),
            ("offset", "Pagination offset"),
        ],
    ),
    ok(
        route(
            HttpMethod::Post,
            "/admin/health/library/scan",
            "Library health",
            "Generate a library health report",
            Auth::Editor,
            Payload::Schema("LibraryHealthStartResponse"),
        ),
        202,
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/admin/health/library/actions",
                "Library health",
                "Apply a safe repair action",
                Auth::Editor,
                Payload::Schema("LibraryHealthActionResult"),
            ),
            "LibraryHealthActionRequest",
        ),
        200,
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
    // Backups (#169)
    route(
        HttpMethod::Get,
        "/admin/backups",
        "Backups",
        "List backup archives and their verification state",
        Auth::Admin,
        Payload::Schema("BackupList"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/admin/backups",
            "Backups",
            "Create a backup archive, or plan one with a dry run",
            Auth::Admin,
            Payload::Schema("BackupCreated"),
        ),
        "CreateBackupRequest",
    ),
    ok(
        route(
            HttpMethod::Post,
            "/admin/backups/{name}/verify",
            "Backups",
            "Verify every checksum in an archive",
            Auth::Admin,
            Payload::Schema("BackupVerification"),
        ),
        200,
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/admin/backups/{name}/restore",
                "Backups",
                "Preview or restore an archive after a safety copy",
                Auth::Admin,
                Payload::Schema("RestoreBackupResponse"),
            ),
            200,
        ),
        "RestoreBackupRequest",
    ),
    query(
        ok(
            route(
                HttpMethod::Delete,
                "/admin/backups/{name}",
                "Backups",
                "Delete an archive after a typed confirmation",
                Auth::Admin,
                Payload::None,
            ),
            204,
        ),
        &[("confirm", "Archive file name; must match the path")],
    ),
    route(
        HttpMethod::Get,
        "/admin/backups/{name}/download",
        "Backups",
        "Stream an archive for an offsite copy",
        Auth::Admin,
        Payload::Binary,
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
    // Observability (#170)
    query(
        route(
            HttpMethod::Get,
            "/admin/observability",
            "Observability",
            "Admin dashboard snapshot",
            Auth::Admin,
            Payload::Schema("ObservabilitySnapshot"),
        ),
        &[("window", "Time window: 1h (default) or 24h")],
    ),
    // Diagnostics (#171)
    body(
        ok(
            route(
                HttpMethod::Post,
                "/diagnostics",
                "Diagnostics",
                "Submit an explicitly sent anonymous report",
                Auth::Public,
                Payload::Object,
            ),
            202,
        ),
        "DiagnosticsReportInput",
    ),
    query(
        route(
            HttpMethod::Get,
            "/diagnostics",
            "Diagnostics",
            "List reports and aggregate counts",
            Auth::Admin,
            Payload::Schema("DiagnosticsPage"),
        ),
        &[
            ("limit", "Page size (1-200, default 50)"),
            ("before", "Cursor"),
            ("kind", "Filter by error kind"),
        ],
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/diagnostics/{id}",
            "Diagnostics",
            "Delete one stored report",
            Auth::Admin,
            Payload::None,
        ),
        204,
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
    body(
        route(
            HttpMethod::Post,
            "/collections/import",
            "Collections",
            "Copy a public or link-shared collection into your account",
            Auth::Session,
            Payload::Schema("ImportCollectionResponse"),
        ),
        "ImportCollectionRequest",
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
    body(
        route(
            HttpMethod::Post,
            "/collections/{id}/share",
            "Collections",
            "Create a share link for a collection",
            Auth::Session,
            Payload::Schema("CollectionShareLink"),
        ),
        "CreateCollectionShareRequest",
    ),
    route(
        HttpMethod::Get,
        "/collections/{id}/shares",
        "Collections",
        "List a collection's share links",
        Auth::Session,
        Payload::Array("CollectionShareLink"),
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/collections/{id}/shares/{share_id}",
            "Collections",
            "Revoke a collection share link",
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
    // Friends, presence and notifications (#161)
    route(
        HttpMethod::Get,
        "/friends",
        "Friends",
        "Friends, pending requests and blocks",
        Auth::Session,
        Payload::Schema("FriendsResponse"),
    ),
    query(
        route(
            HttpMethod::Get,
            "/friends/activity",
            "Friends",
            "Activity of friends that opted in",
            Auth::Session,
            Payload::Schema("ActivityPage"),
        ),
        &[("limit", "Page size"), ("before", "Cursor")],
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/friends/requests",
                "Friends",
                "Send a friend request by exact username or id",
                Auth::Session,
                Payload::Schema("FriendRequest"),
            ),
            201,
        ),
        "FriendRequestInput",
    ),
    route(
        HttpMethod::Post,
        "/friends/requests/{id}/accept",
        "Friends",
        "Accept an incoming friend request",
        Auth::Session,
        Payload::Schema("Friend"),
    ),
    ok(
        route(
            HttpMethod::Post,
            "/friends/requests/{id}/decline",
            "Friends",
            "Decline an incoming friend request",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/friends/requests/{id}",
            "Friends",
            "Cancel an outgoing friend request",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/friends/{id}",
            "Friends",
            "Remove a friend",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Get,
        "/blocks",
        "Friends",
        "Accounts you blocked",
        Auth::Session,
        Payload::Array("SocialUser"),
    ),
    ok(
        route(
            HttpMethod::Post,
            "/blocks/{id}",
            "Friends",
            "Block an account",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/blocks/{id}",
            "Friends",
            "Lift a block",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Get,
        "/presence",
        "Friends",
        "Your presence and opt-in state",
        Auth::Session,
        Payload::Schema("PresenceStateResponse"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/presence",
            "Friends",
            "Presence heartbeat",
            Auth::Session,
            Payload::Schema("PresenceStateResponse"),
        ),
        "PresenceHeartbeat",
    ),
    route(
        HttpMethod::Get,
        "/presence/friends",
        "Friends",
        "Online friends, for bounded polling",
        Auth::Session,
        Payload::Schema("FriendsPresenceResponse"),
    ),
    query(
        route(
            HttpMethod::Get,
            "/notifications",
            "Notifications",
            "In-app notifications, newest first",
            Auth::Session,
            Payload::Schema("NotificationPage"),
        ),
        &[("limit", "Page size"), ("before", "Cursor")],
    ),
    route(
        HttpMethod::Post,
        "/notifications/read-all",
        "Notifications",
        "Mark every notification read",
        Auth::Session,
        Payload::Schema("NotificationReadResponse"),
    ),
    route(
        HttpMethod::Post,
        "/notifications/{id}/read",
        "Notifications",
        "Mark one notification read",
        Auth::Session,
        Payload::Schema("NotificationReadResponse"),
    ),
    route(
        HttpMethod::Get,
        "/me/notification-prefs",
        "Notifications",
        "Notification preferences",
        Auth::Session,
        Payload::Schema("NotificationPrefs"),
    ),
    body(
        route(
            HttpMethod::Patch,
            "/me/notification-prefs",
            "Notifications",
            "Update notification preferences",
            Auth::Session,
            Payload::Schema("NotificationPrefs"),
        ),
        "NotificationPrefsPatch",
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
    route(
        HttpMethod::Get,
        "/share/collection/{token}",
        "Shares",
        "Public metadata for a collection share link",
        Auth::Public,
        Payload::Schema("CollectionShareView"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/share/collection/{token}/access",
            "Shares",
            "Unlock a password-protected collection share link",
            Auth::Public,
            Payload::Schema("CollectionShareView"),
        ),
        "CollectionShareAccessRequest",
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
    // Controller profiles (#51)
    route(
        HttpMethod::Get,
        "/controller-profiles",
        "Controller profiles",
        "Controller profiles of the current account and built-in presets",
        Auth::Session,
        Payload::Schema("ControllerProfilesResponse"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/controller-profiles",
            "Controller profiles",
            "Save the account-wide or a per-game controller profile",
            Auth::Session,
            Payload::Schema("ControllerProfileEntry"),
        ),
        "SaveControllerProfileRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/controller-profiles/{gameId}",
            "Controller profiles",
            "Delete a controller profile scope (0 = account-wide)",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    // Emulators and ROMs (#52)
    route(
        HttpMethod::Get,
        "/emulators",
        "Emulators",
        "List configured emulators",
        Auth::Session,
        Payload::Array("Emulator"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/emulators",
            "Emulators",
            "Register an emulator command",
            Auth::Admin,
            Payload::Schema("Emulator"),
        ),
        "CreateEmulatorRequest",
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/emulators/validate",
                "Emulators",
                "Validate an emulator command template without running it",
                Auth::Admin,
                Payload::Schema("ValidateCommandResponse"),
            ),
            "ValidateCommandRequest",
        ),
        200,
    ),
    route(
        HttpMethod::Get,
        "/emulators/defaults",
        "Emulators",
        "Default emulator per platform",
        Auth::Session,
        Payload::Array("PlatformDefault"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/emulators/defaults",
            "Emulators",
            "Set or clear platform default emulators",
            Auth::Admin,
            Payload::Array("PlatformDefault"),
        ),
        "UpdateDefaultsRequest",
    ),
    body(
        route(
            HttpMethod::Patch,
            "/emulators/{id}",
            "Emulators",
            "Update an emulator",
            Auth::Admin,
            Payload::Schema("Emulator"),
        ),
        "UpdateEmulatorRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/emulators/{id}",
            "Emulators",
            "Delete an emulator",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Get,
        "/platforms",
        "Emulators",
        "Platform catalog with ROM counts",
        Auth::Session,
        Payload::Array("PlatformView"),
    ),
    ok(
        route(
            HttpMethod::Post,
            "/games/{id}/launch-command",
            "Emulators",
            "Resolve the emulator launch command for a ROM game",
            Auth::Session,
            Payload::Schema("EmulatorLaunch"),
        ),
        200,
    ),
    ok(
        body(
            route(
                HttpMethod::Put,
                "/games/{id}/emulator",
                "Emulators",
                "Set or clear the per-game emulator override",
                Auth::Editor,
                Payload::None,
            ),
            "SetGameEmulatorRequest",
        ),
        204,
    ),
    // Device registry (#184)
    route(
        HttpMethod::Get,
        "/devices",
        "Devices",
        "List the caller's registered devices with their online state",
        Auth::Session,
        Payload::Array("Device"),
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/devices/heartbeat",
                "Devices",
                "Register this device or refresh its presence and capabilities",
                Auth::Session,
                Payload::Schema("Device"),
            ),
            "DeviceHeartbeatRequest",
        ),
        200,
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/devices/{id}",
            "Devices",
            "Revoke a device; it can no longer heartbeat or claim installs",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    // Mobile companion (#167)
    query(
        route(
            HttpMethod::Get,
            "/remote-installs",
            "Remote installs",
            "List the caller's remote install requests",
            Auth::Session,
            Payload::Array("RemoteInstall"),
        ),
        &[
            (
                "status",
                "queued, claimed, downloading, ready, failed or cancelled; comma-separate several",
            ),
            ("limit", "1-200, default 100"),
        ],
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/remote-installs",
                "Remote installs",
                "Queue an install for a signed-in desktop (idempotent per game)",
                Auth::Session,
                Payload::Schema("RemoteInstall"),
            ),
            "CreateRemoteInstallRequest",
        ),
        201,
    ),
    route(
        HttpMethod::Get,
        "/remote-installs/{id}",
        "Remote installs",
        "Read one remote install request",
        Auth::Session,
        Payload::Schema("RemoteInstall"),
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/remote-installs/{id}",
            "Remote installs",
            "Cancel an active request or clear a finished one",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/remote-installs/{id}/status",
                "Remote installs",
                "Report desktop progress for a remote install",
                Auth::Session,
                Payload::Schema("RemoteInstall"),
            ),
            "RemoteInstallStatusRequest",
        ),
        200,
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
        if matches!(
            doc.path,
            "/auth/login" | "/auth/login/mfa" | "/invitations/{id}/accept"
        ) {
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
        .schema_from::<crate::models::User>()
        .schema_from::<crate::models::UserSource>()
        .schema_from::<ludex_core::models::GameSummary>()
        .schema_from::<ludex_core::models::GameDetail>()
        .schema_from::<ludex_core::models::GameVersion>()
        .schema_from::<ludex_core::models::LatestVersion>()
        .schema_from::<crate::models::GameVersionInfo>()
        .schema_from::<crate::models::GameVersionsResponse>()
        .schema_from::<crate::models::SetDefaultVersionRequest>()
        .schema_from::<crate::models::DeleteVersionResponse>()
        .schema_from::<ludex_core::archive::StreamManifest>()
        .schema_from::<ludex_core::archive::StreamEntry>()
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
        .schema_from::<crate::observability::ObservabilitySnapshot>()
        .schema_from::<crate::observability::ObservabilityServer>()
        .schema_from::<crate::observability::ObservabilityRequests>()
        .schema_from::<crate::observability::ObservabilityPoint>()
        .schema_from::<crate::observability::ObservabilityErrorRoute>()
        .schema_from::<crate::observability::ObservabilityActive>()
        .schema_from::<crate::observability::ObservabilityDatabase>()
        .schema_from::<crate::observability::ObservabilityDisk>()
        .schema_from::<crate::observability::ObservabilityMedia>()
        .schema_from::<crate::observability::ObservabilitySaves>()
        .schema_from::<crate::observability::ObservabilityBackups>()
        .schema_from::<crate::observability::ObservabilityRoot>()
        .schema_from::<crate::observability::ObservabilityStorage>()
        .schema_from::<crate::observability::ObservabilityEvent>()
        .schema_from::<crate::observability::ObservabilityWebhookFailure>()
        .schema_from::<crate::observability::ObservabilityWebhooks>()
        .schema_from::<crate::observability::ObservabilityFinding>()
        .schema_from::<crate::observability::ObservabilityIntegrity>()
        .schema_from::<crate::observability::ObservabilityPrometheus>()
        .schema_from::<crate::models::ServerConfig>()
        .schema_from::<crate::desktop_download::DesktopDownload>()
        .schema_from::<crate::desktop_download::DesktopAsset>()
        .schema_from::<crate::models::FeatureFlags>()
        .schema_from::<crate::models::ChangePasswordRequest>()
        .schema_from::<crate::models::OnboardingRequest>()
        .schema_from::<crate::models::AuthSession>()
        .schema_from::<crate::models::MfaLoginRequest>()
        .schema_from::<crate::models::MfaStatus>()
        .schema_from::<crate::models::TotpEnrollment>()
        .schema_from::<crate::models::TotpActivationRequest>()
        .schema_from::<crate::models::TotpActivated>()
        .schema_from::<crate::models::MfaDisableRequest>()
        .schema_from::<crate::models::RecoveryCodesRequest>()
        .schema_from::<crate::models::RecoveryCodes>()
        .schema_from::<crate::models::Invitation>()
        .schema_from::<crate::models::InvitationStatus>()
        .schema_from::<crate::models::CreateInvitationRequest>()
        .schema_from::<crate::models::CreatedInvitation>()
        .schema_from::<crate::models::InvitationPreview>()
        .schema_from::<crate::models::AcceptInvitationRequest>()
        .schema_from::<crate::models::ResetPasswordRequest>()
        .schema_from::<crate::models::ResetPasswordResult>()
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
        .schema_from::<crate::providers::MetadataProviderView>()
        .schema_from::<crate::providers::ProviderCandidate>()
        .schema_from::<crate::providers::ProviderTestResponse>()
        .schema_from::<crate::routes::metadata_providers::CreateMetadataProviderRequest>()
        .schema_from::<crate::routes::metadata_providers::UpdateMetadataProviderRequest>()
        .schema_from::<crate::routes::metadata_providers::ReorderMetadataProvidersRequest>()
        .schema_from::<crate::routes::metadata_providers::TestMetadataProviderRequest>()
        .schema_from::<crate::routes::game_metadata::MetadataCandidate>()
        .schema_from::<crate::routes::game_metadata::MetadataProviderStatus>()
        .schema_from::<crate::routes::game_metadata::MetadataCandidatesResponse>()
        .schema_from::<crate::routes::game_metadata::ApplyMetadataRequest>()
        .schema_from::<crate::models::LibraryFolders>()
        .schema_from::<crate::roots::LibraryRootView>()
        .schema_from::<crate::models::CreateLibraryRootRequest>()
        .schema_from::<crate::models::UpdateLibraryRootRequest>()
        .schema_from::<ludex_core::gamepad::ControllerProfile>()
        .schema_from::<crate::controller_profiles::ControllerProfileEntry>()
        .schema_from::<crate::controller_profiles::ControllerProfilesResponse>()
        .schema_from::<crate::controller_profiles::PresetEntry>()
        .schema_from::<crate::controller_profiles::SaveControllerProfileRequest>()
        .schema_from::<crate::emulators::Emulator>()
        .schema_from::<crate::emulators::CreateEmulatorRequest>()
        .schema_from::<crate::emulators::UpdateEmulatorRequest>()
        .schema_from::<crate::emulators::PlatformView>()
        .schema_from::<crate::emulators::PlatformDefault>()
        .schema_from::<crate::emulators::UpdateDefaultsRequest>()
        .schema_from::<crate::emulators::ValidateCommandRequest>()
        .schema_from::<crate::emulators::ValidateCommandResponse>()
        .schema_from::<crate::emulators::SetGameEmulatorRequest>()
        .schema_from::<crate::emulators::EmulatorLaunch>()
        .schema_from::<crate::models::RootScanResponse>()
        .schema_from::<crate::library_health::LibraryHealthCounts>()
        .schema_from::<crate::library_health::LibraryHealthCategoryCount>()
        .schema_from::<crate::library_health::LibraryHealthReport>()
        .schema_from::<crate::library_health::LibraryHealthFinding>()
        .schema_from::<crate::library_health::LibraryHealthResponse>()
        .schema_from::<crate::library_health::LibraryHealthStartResponse>()
        .schema_from::<crate::library_health::LibraryHealthActionRequest>()
        .schema_from::<crate::library_health::LibraryHealthActionResult>()
        .schema_from::<crate::library_health::LibraryHealthMediaGc>()
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
        .schema_from::<crate::models::DiagnosticsReportInput>()
        .schema_from::<crate::models::DiagnosticsReport>()
        .schema_from::<crate::models::DiagnosticsCount>()
        .schema_from::<crate::models::DiagnosticsPage>()
        .schema_from::<crate::models::Webhook>()
        .schema_from::<crate::models::CreateWebhookRequest>()
        .schema_from::<crate::models::CreateWebhookResponse>()
        .schema_from::<crate::models::UpdateWebhookRequest>()
        .schema_from::<crate::models::UpdateWebhookResponse>()
        .schema_from::<crate::models::WebhookTestResponse>()
        .schema_from::<crate::models::Collection>()
        .schema_from::<crate::models::CollectionKind>()
        .schema_from::<crate::models::CollectionVisibility>()
        .schema_from::<crate::models::CreateCollectionRequest>()
        .schema_from::<crate::models::UpdateCollectionRequest>()
        .schema_from::<crate::models::CollectionGamesRequest>()
        .schema_from::<crate::models::CollectionGamesResponse>()
        .schema_from::<crate::collection_share::CreateCollectionShareRequest>()
        .schema_from::<crate::collection_share::CollectionShareAccessRequest>()
        .schema_from::<crate::collection_share::CollectionShareLink>()
        .schema_from::<crate::collection_share::CollectionShareView>()
        .schema_from::<crate::collection_share::SharedCollection>()
        .schema_from::<crate::collection_share::SharedCollectionOwner>()
        .schema_from::<crate::collection_share::SharedCollectionGame>()
        .schema_from::<crate::collection_share::ImportCollectionRequest>()
        .schema_from::<crate::collection_share::ImportCollectionResponse>()
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
        .schema_from::<crate::social::SocialUser>()
        .schema_from::<crate::social::FriendGame>()
        .schema_from::<crate::social::FriendPresence>()
        .schema_from::<crate::social::Friend>()
        .schema_from::<crate::social::FriendRequest>()
        .schema_from::<crate::social::FriendsResponse>()
        .schema_from::<crate::social::FriendRequestInput>()
        .schema_from::<crate::social::FriendPresenceEntry>()
        .schema_from::<crate::social::FriendsPresenceResponse>()
        .schema_from::<crate::social::PresenceStateResponse>()
        .schema_from::<crate::social::PresenceHeartbeat>()
        .schema_from::<crate::social::NotificationAchievement>()
        .schema_from::<crate::social::Notification>()
        .schema_from::<crate::social::NotificationPage>()
        .schema_from::<crate::social::NotificationPrefs>()
        .schema_from::<crate::social::NotificationPrefsPatch>()
        .schema_from::<crate::social::NotificationReadResponse>()
        .schema_from::<crate::models::AuditLog>()
        .schema_from::<crate::models::SuggestResponse>()
        .schema_from::<crate::models::GameSuggestion>()
        .schema_from::<crate::routes::oidc::OidcExchangeRequest>()
        .schema_from::<crate::routes::oidc::DesktopHandoffRequest>()
        .schema_from::<crate::routes::oidc::DesktopHandoffResponse>()
        .schema_from::<crate::routes::oidc::DesktopHandoffClaimRequest>()
        .schema_from::<crate::routes::oidc::DesktopHandoffClaimResponse>()
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
        .schema_from::<crate::models::Device>()
        .schema_from::<crate::models::DeviceCapabilities>()
        .schema_from::<crate::models::DeviceHeartbeatRequest>()
        .schema_from::<crate::remote_installs::RemoteInstall>()
        .schema_from::<crate::remote_installs::RemoteInstallStatus>()
        .schema_from::<crate::remote_installs::CreateRemoteInstallRequest>()
        .schema_from::<crate::remote_installs::RemoteInstallStatusRequest>()
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
        .schema_from::<crate::routes::backups::BackupEntry>()
        .schema_from::<crate::routes::backups::BackupList>()
        .schema_from::<crate::routes::backups::BackupVerification>()
        .schema_from::<crate::routes::backups::CreateBackupRequest>()
        .schema_from::<crate::routes::backups::BackupCreated>()
        .schema_from::<crate::routes::backups::RestoreBackupRequest>()
        .schema_from::<crate::routes::backups::RestoreBackupResponse>()
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
        (
            "Authentication",
            "Login, sessions, second factors and passwords",
        ),
        ("Users", "Administrator account management"),
        (
            "Invitations",
            "Expiring single-use links that create local accounts",
        ),
        ("Games", "Library browsing, metadata and downloads"),
        ("Search", "Fuzzy title search"),
        ("Library", "Scanning, roots and the folder watcher"),
        ("Integrity", "Archive verification reports"),
        (
            "Backups",
            "Backup archives, verification, download and restore",
        ),
        ("Media", "Cover, background and screenshot cache"),
        ("News", "Server news posts"),
        ("Cloud saves", "Per-user save snapshots"),
        ("API keys", "Bearer API keys for integrations"),
        ("Audit", "Persistent audit trail"),
        ("Observability", "Admin health and activity dashboard"),
        (
            "Diagnostics",
            "Opt-in anonymous crash and error reports (#171)",
        ),
        ("Webhooks", "Outbound HMAC-signed events"),
        ("Collections", "Curated and smart collections"),
        ("Community", "Public profiles and activity"),
        ("Friends", "Friend requests, presence and blocks"),
        (
            "Notifications",
            "In-app notifications and per-kind preferences",
        ),
        ("Shares", "Public, unauthenticated share links"),
        ("Uploads", "Resumable archive uploads"),
        ("Ingest", "URL download manager"),
        (
            "Remote installs",
            "Phone-queued installs for a signed-in desktop",
        ),
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
