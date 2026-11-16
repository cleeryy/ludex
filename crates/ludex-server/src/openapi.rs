//! Generated OpenAPI 3.1 description of the Ludex HTTP API.
//!
//! Every route module declares the operations it serves in its own `DOCS`
//! table, next to its `router()` (#237); [`ROUTES`] only concatenates them in
//! documentation order. The contract test compares that table against the axum
//! router source. Each entry names the request and response schemas that the
//! handler actually uses; schemas come from `utoipa::ToSchema` derives on the
//! DTOs, so the document cannot drift from the types. `tests/openapi.rs` fails
//! when a registered route is missing from the table, when a path is
//! documented twice, or when a referenced schema is not registered.

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

pub(crate) const fn route(
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

pub(crate) const fn body(mut route: RouteDoc, request: &'static str) -> RouteDoc {
    route.request = Some(request);
    route
}

pub(crate) const fn ok(mut route: RouteDoc, status: u16) -> RouteDoc {
    route.status = status;
    route
}

pub(crate) const fn query(
    mut route: RouteDoc,
    params: &'static [(&'static str, &'static str)],
) -> RouteDoc {
    route.query = params;
    route
}

/// Every route served under `/api` plus the ingest API.
///
/// Concatenated from each module's `DOCS` table; the order is documentation
/// order and the contract test treats the result as a set.
pub static ROUTES: std::sync::LazyLock<Vec<RouteDoc>> = std::sync::LazyLock::new(|| {
    let mut routes = Vec::new();
    routes.extend_from_slice(crate::routes::system::DOCS);
    routes.extend_from_slice(crate::routes::admin_oidc::DOCS);
    routes.extend_from_slice(crate::routes::admin_server::DOCS);
    routes.extend_from_slice(crate::routes::metadata_providers::DOCS);
    routes.extend_from_slice(crate::routes::auth::DOCS);
    routes.extend_from_slice(crate::routes::account::DOCS);
    routes.extend_from_slice(crate::routes::invitations::DOCS);
    routes.extend_from_slice(crate::routes::oidc::DOCS);
    routes.extend_from_slice(crate::routes::users::DOCS);
    routes.extend_from_slice(crate::routes::games::DOCS);
    routes.extend_from_slice(crate::routes::search::DOCS);
    routes.extend_from_slice(crate::routes::library::DOCS);
    routes.extend_from_slice(crate::routes::admin_games::DOCS);
    routes.extend_from_slice(crate::routes::library_health::DOCS);
    routes.extend_from_slice(crate::routes::integrity::DOCS);
    routes.extend_from_slice(crate::routes::backups::DOCS);
    routes.extend_from_slice(crate::routes::media::DOCS);
    routes.extend_from_slice(crate::routes::news::DOCS);
    routes.extend_from_slice(crate::routes::saves::DOCS);
    routes.extend_from_slice(crate::routes::keys::DOCS);
    routes.extend_from_slice(crate::routes::audit::DOCS);
    routes.extend_from_slice(crate::routes::observability::DOCS);
    routes.extend_from_slice(crate::routes::diagnostics::DOCS);
    routes.extend_from_slice(crate::routes::webhooks::DOCS);
    routes.extend_from_slice(crate::routes::collections::DOCS);
    routes.extend_from_slice(crate::routes::community::DOCS);
    routes.extend_from_slice(crate::routes::social::DOCS);
    routes.extend_from_slice(crate::routes::artwork::DOCS);
    routes.extend_from_slice(crate::routes::share::DOCS);
    routes.extend_from_slice(crate::routes::uploads::DOCS);
    routes.extend_from_slice(crate::ingest::DOCS);
    routes.extend_from_slice(crate::routes::achievements::DOCS);
    routes.extend_from_slice(crate::routes::controllers::DOCS);
    routes.extend_from_slice(crate::routes::emulators::DOCS);
    routes.extend_from_slice(crate::routes::devices::DOCS);
    routes.extend_from_slice(crate::routes::remote_installs::DOCS);
    routes.extend_from_slice(crate::routes::virustotal::DOCS);
    routes.extend_from_slice(crate::routes::versions::DOCS);
    routes
});

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
    for doc in ROUTES.iter() {
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
        .schema_from::<crate::routes::account::AccountProfileRequest>()
        .schema_from::<crate::routes::preferences::PreferencesResponse>()
        .schema_from::<crate::routes::preferences::ReplacePreferencesRequest>()
        .schema_from::<crate::routes::preferences::PatchPreferencesRequest>()
        .schema_from::<crate::routes::admin_server::ServerSettings>()
        .schema_from::<crate::routes::admin_server::UpdateServerSettingsRequest>()
        .schema_from::<crate::models::UserSource>()
        .schema_from::<ludex_core::models::GameSummary>()
        .schema_from::<ludex_core::models::GameDetail>()
        .schema_from::<ludex_core::models::GameVersion>()
        .schema_from::<ludex_core::models::LatestVersion>()
        .schema_from::<crate::models::GameVersionInfo>()
        .schema_from::<crate::models::GameVersionsResponse>()
        .schema_from::<crate::models::SetDefaultVersionRequest>()
        .schema_from::<crate::models::DeleteVersionResponse>()
        .schema_from::<crate::extracted_cache::ExtractState>()
        .schema_from::<crate::extracted_cache::ExtractStatus>()
        .schema_from::<crate::extracted_cache::ExtractedFile>()
        .schema_from::<crate::routes::versions::ManifestResponse>()
        .schema_from::<crate::routes::versions::CacheOverview>()
        .schema_from::<crate::routes::versions::CacheEntryView>()
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
        .schema_from::<crate::routes::games::DeleteGameResponse>()
        .schema_from::<crate::routes::admin_games::AdminGameRow>()
        .schema_from::<crate::routes::admin_games::AdminGameCounts>()
        .schema_from::<crate::routes::admin_games::AdminGamesResponse>()
        .schema_from::<crate::routes::admin_games::BulkRequest>()
        .schema_from::<crate::routes::admin_games::BulkPreviewItem>()
        .schema_from::<crate::routes::admin_games::BulkPreview>()
        .schema_from::<crate::routes::admin_games::BulkItemResult>()
        .schema_from::<crate::routes::admin_games::BulkTotals>()
        .schema_from::<crate::routes::admin_games::BulkResponse>()
        .schema_from::<crate::routes::admin_games::BulkJobView>()
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
        .schema_from::<crate::content_hash::ContentHashStatus>()
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
        .schema_from::<crate::virustotal::IntegrationStatus>()
        .schema_from::<crate::virustotal::QuotaStatus>()
        .schema_from::<crate::virustotal::KeySource>()
        .schema_from::<crate::virustotal::TestResponse>()
        .schema_from::<crate::virustotal::ScanRequest>()
        .schema_from::<crate::routes::virustotal::ScanResponse>()
        .schema_from::<crate::virustotal::ScanReport>()
        .schema_from::<crate::virustotal::GameScanReport>()
        .schema_from::<crate::virustotal::Verdict>()
        .schema_from::<crate::virustotal::VtStats>()
        .schema_from::<crate::virustotal::EngineHit>()
        .schema_from::<crate::virustotal::ScanSource>()
        .schema_from::<crate::virustotal::ScanMode>()
        .schema_from::<crate::routes::virustotal::SaveVirustotalKeyRequest>()
        .schema_from::<crate::virustotal_rescan::RescanStatus>()
        .schema_from::<crate::virustotal_rescan::RescanPolicyInput>()
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
        (
            "VirusTotal",
            "Archive hash lookups, the encrypted API key and the daily quota",
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
