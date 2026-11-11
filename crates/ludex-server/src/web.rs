//! Optional web UI serving: an on-disk bundle (`LUDEX_WEB_DIR`) takes
//! precedence over the `embed-web` build-time feature.

#[cfg(not(feature = "embed-web"))]
use axum::extract::State;
use axum::response::Redirect;
use axum::routing::get;
use axum::Router;
#[cfg(not(feature = "embed-web"))]
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};

use crate::state::AppState;

/// Attaches the web UI (or a small JSON banner) as the router fallback.
///
/// `/download` is a plain path, not a hash route: it redirects to the public
/// SPA route `/#/download` so a link copied from the README works before the
/// bundle has loaded (#164).
pub fn attach(app: Router<AppState>, state: &AppState) -> Router<AppState> {
    let app = app.route("/download", get(download_redirect));

    if let Some(directory) = state.config.web_dir.as_deref() {
        let index = directory.join("index.html");
        if directory.is_dir() && index.is_file() {
            let service = ServeDir::new(directory).fallback(ServeFile::new(index));
            return app.fallback_service(service);
        }
        tracing::warn!(
            path = %directory.display(),
            "LUDEX_WEB_DIR does not contain index.html; falling back to the embedded UI"
        );
    }

    #[cfg(feature = "embed-web")]
    let app = app.fallback(get(embedded::asset));

    #[cfg(not(feature = "embed-web"))]
    let app = app.fallback(get(root_info));

    app
}

async fn download_redirect() -> Redirect {
    Redirect::temporary("/#/download")
}

#[cfg(not(feature = "embed-web"))]
async fn root_info(State(state): State<AppState>) -> axum::Json<serde_json::Value> {
    let name = crate::db::server_name(&state.pool, &state.config.server_name).await;
    axum::Json(json!({
        "name": name,
        "version": env!("CARGO_PKG_VERSION"),
        "api": "/api/health",
    }))
}

#[cfg(feature = "embed-web")]
mod embedded {
    use axum::body::Body;
    use axum::http::{header, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use rust_embed::RustEmbed;

    #[derive(RustEmbed)]
    #[folder = "../../apps/web/dist"]
    struct Assets;

    pub async fn asset(uri: Uri) -> Response {
        let requested = uri.path().trim_start_matches('/');
        let requested = if requested.is_empty() {
            "index.html"
        } else {
            requested
        };
        match Assets::get(requested) {
            Some(file) => {
                let mime = mime_guess::from_path(requested)
                    .first_or_octet_stream()
                    .to_string();
                match Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime)
                    .body(Body::from(file.data.into_owned()))
                {
                    Ok(response) => response,
                    Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                }
            }
            None => match Assets::get("index.html") {
                Some(file) => match Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html")
                    .body(Body::from(file.data.into_owned()))
                {
                    Ok(response) => response,
                    Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                },
                None => StatusCode::NOT_FOUND.into_response(),
            },
        }
    }
}
