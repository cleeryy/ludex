//! OpenAPI discovery routes (#17).
//!
//! `GET /api/openapi.json` is always public: it is generated from the route
//! table in [`crate::openapi`] and the DTO schemas. The human-readable
//! explorer at `/api/docs` is opt-in (`LUDEX_OPENAPI_DOCS=true`) so a public
//! deployment does not have to expose it.

use axum::extract::State;
use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::error::ApiError;
use crate::openapi;
use crate::state::AppState;

/// OpenAPI routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/openapi.json", get(document))
        .route("/docs", get(docs))
}

async fn document() -> Response {
    let mut response = Json(openapi::document()).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn docs(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    if !state.config.openapi_docs {
        return Err(ApiError::not_found(
            "docs_disabled",
            "The API explorer is disabled; set LUDEX_OPENAPI_DOCS=true to enable it",
        ));
    }
    Ok(Html(
        DOCS_HTML.replace("{{VERSION}}", env!("CARGO_PKG_VERSION")),
    ))
}

/// Minimal explorer page. Loads the generated document from the same origin.
const DOCS_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Ludex API {{VERSION}}</title>
<style>
  html { scroll-behavior: smooth; }
  body { margin: 0; font-family: ui-sans-serif, system-ui, sans-serif; background: #101010; color: #f2f2f2; }
  header { padding: 20px 24px; border-bottom: 1px solid #2a2a2a; display: flex; gap: 16px; align-items: baseline; }
  header h1 { font-size: 18px; margin: 0; letter-spacing: .02em; }
  header span { color: #9a9a9a; font-size: 12px; }
  main { padding: 24px; max-width: 960px; margin: 0 auto; }
  .op { border: 1px solid #2a2a2a; border-radius: 8px; margin-bottom: 10px; padding: 12px 16px; }
  .method { display: inline-block; min-width: 64px; font-weight: 600; font-size: 12px; letter-spacing: .06em; }
  .path { font-family: ui-monospace, monospace; font-size: 13px; }
  .summary { color: #b8b8b8; font-size: 13px; margin-top: 6px; }
  a { color: inherit; }
</style>
</head>
<body>
<header>
  <h1>Ludex API</h1>
  <span>v{{VERSION}} · generated from the server route table</span>
  <span><a href="openapi.json">openapi.json</a></span>
</header>
<main id="ops">Loading…</main>
<script>
fetch("openapi.json").then(r => r.json()).then(doc => {
  const root = document.getElementById("ops");
  root.textContent = "";
  const entries = [];
  for (const [path, item] of Object.entries(doc.paths || {})) {
    for (const method of ["get","post","put","patch","delete"]) {
      if (item[method]) entries.push([method, path, item[method]]);
    }
  }
  entries.sort((a, b) => a[1].localeCompare(b[1]) || a[0].localeCompare(b[0]));
  for (const [method, path, op] of entries) {
    const row = document.createElement("div");
    row.className = "op";
    const tag = (op.tags && op.tags[0]) || "";
    const m = document.createElement("span");
    m.className = "method";
    m.textContent = method.toUpperCase();
    const p = document.createElement("span");
    p.className = "path";
    p.textContent = path;
    const s = document.createElement("div");
    s.className = "summary";
    s.textContent = (tag ? tag + " — " : "") + (op.summary || "");
    row.append(m, p, s);
    root.append(row);
  }
});
</script>
</body>
</html>
"#;
