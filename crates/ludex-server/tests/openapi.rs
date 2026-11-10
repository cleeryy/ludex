//! Contract tests for `GET /api/openapi.json` (#17).
//!
//! The OpenAPI document is generated from `ludex_server::openapi::ROUTES`. This
//! test parses every axum `.route(...)` registration in the server source and
//! fails when the router and the document disagree, then probes each documented
//! path against a live server to prove it is really mounted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludex_server::{start, Config};
use reqwest::{Client, Method, StatusCode};
use serde_json::Value;

fn source_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("routes"))
        .expect("read routes dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.push(root.join("ingest").join("mod.rs"));
    files.sort();
    files
}

/// Parses `.route("...", get(a).post(b))` registrations from Rust source.
fn registered_routes(source: &str) -> Vec<(String, String)> {
    let mut routes = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0;
    while let Some(found) = source[index..].find(".route(") {
        let start = index + found + ".route(".len();
        let mut cursor = start;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'"' {
            index = start;
            continue;
        }
        // Path literal.
        let path_start = cursor + 1;
        let Some(path_end_rel) = source[path_start..].find('"') else {
            break;
        };
        let path_end = path_start + path_end_rel;
        let path = &source[path_start..path_end];
        // Matching closing parenthesis of the `.route(` call.
        let mut depth = 1usize;
        let mut position = path_end + 1;
        let mut in_string = false;
        let mut escaped = false;
        while position < bytes.len() && depth > 0 {
            let byte = bytes[position];
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
            } else if byte == b'"' {
                in_string = true;
            } else if byte == b'(' {
                depth += 1;
            } else if byte == b')' {
                depth -= 1;
            }
            position += 1;
        }
        let expression = &source[path_end + 1..position.saturating_sub(1)];
        for method in ["get", "post", "put", "patch", "delete"] {
            if contains_call(expression, method) {
                routes.push((method.to_ascii_uppercase(), normalize(path)));
            }
        }
        index = position.max(start);
    }
    routes
}

/// Whether `expression` contains a `name(` call at a token boundary.
fn contains_call(expression: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    let mut index = 0;
    while let Some(found) = expression[index..].find(&needle) {
        let position = index + found;
        let boundary = position == 0
            || expression[..position]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_alphanumeric() && character != '_');
        if boundary {
            return true;
        }
        index = position + needle.len();
    }
    false
}

/// Documented paths are relative to `/api`; ingest registers absolute paths.
fn normalize(path: &str) -> String {
    path.strip_prefix("/api").unwrap_or(path).to_string()
}

#[test]
fn every_registered_route_is_documented() {
    let mut registered = BTreeSet::new();
    for file in source_files() {
        let source = std::fs::read_to_string(&file).expect("read source");
        for route in registered_routes(&source) {
            assert!(
                registered.insert(route.clone()),
                "duplicate registration {route:?} in {}",
                file.display()
            );
        }
    }
    assert!(
        registered.len() > 60,
        "route parser found too few routes: {registered:?}"
    );

    let documented: BTreeSet<(String, String)> = ludex_server::openapi::documented_operations()
        .into_iter()
        .collect();
    assert_eq!(
        documented.len(),
        ludex_server::openapi::ROUTES.len(),
        "a route is documented twice"
    );

    let missing: Vec<_> = registered.difference(&documented).collect();
    let phantom: Vec<_> = documented.difference(&registered).collect();
    assert!(
        missing.is_empty(),
        "registered routes missing from the OpenAPI document: {missing:?}"
    );
    assert!(
        phantom.is_empty(),
        "OpenAPI documents routes that are not registered in the router: {phantom:?}"
    );
}

#[test]
fn every_schema_reference_resolves() {
    let document = serde_json::to_value(ludex_server::openapi::document()).expect("serialize");
    let schemas = document["components"]["schemas"]
        .as_object()
        .expect("components.schemas");
    assert!(
        document["components"]["securitySchemes"]["bearerAuth"].is_object(),
        "bearerAuth security scheme is missing"
    );

    let mut references = Vec::new();
    collect_refs(&document, &mut references);
    assert!(
        !references.is_empty(),
        "the document has no schema references"
    );
    for reference in references {
        let name = reference
            .strip_prefix("#/components/schemas/")
            .unwrap_or_else(|| panic!("unexpected $ref {reference}"));
        assert!(
            schemas.contains_key(name),
            "schema {name} is referenced but not registered"
        );
    }
    assert!(
        document["paths"]
            .as_object()
            .is_some_and(|paths| !paths.is_empty()),
        "the document has no paths"
    );
}

fn collect_refs(value: &Value, refs: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "$ref" {
                    if let Some(reference) = value.as_str() {
                        refs.push(reference.to_string());
                    }
                } else {
                    collect_refs(value, refs);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_refs(item, refs)),
        _ => {}
    }
}

#[tokio::test]
async fn documented_paths_are_mounted() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::for_local(
        temp.path().join("games"),
        temp.path().join("data"),
        temp.path().join("media"),
    );
    config.local_mode = false;
    config.bind = "127.0.0.1:0".parse().expect("bind");
    config.rate_limit_rps = 0;
    let handle = start(config).await.expect("server starts");
    let client = Client::builder().build().expect("client");

    let mut checked = 0;
    for (method, path) in ludex_server::openapi::documented_operations() {
        let parameters: Vec<String> = ludex_server::openapi::path_parameters(&path)
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut path = path;
        for parameter in parameters {
            path = path.replace(&format!("{{{parameter}}}"), "1");
        }
        // PATCH is only registered on a handful of paths; any other registered
        // path answers 405. An unknown path answers 404 with code `not_found`.
        let response = client
            .request(Method::PATCH, format!("{}/api{}", handle.url, path))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let unknown = status == StatusCode::NOT_FOUND
            && body.contains("\"code\":\"not_found\"")
            && body.contains("Endpoint not found");
        assert!(
            !unknown,
            "documented route {method} /api{path} is not mounted (404 endpoint not found)"
        );
        checked += 1;
    }
    assert!(checked > 60, "only probed {checked} routes");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let _ = handle.shutdown.send(());
}
