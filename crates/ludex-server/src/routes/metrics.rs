//! `GET /metrics` — Prometheus text exposition (#48).
//!
//! Protected by `LUDEX_METRICS_TOKEN` (bearer) when set. Without a token the
//! endpoint is public but limited to 30 scrapes per minute per client IP, so a
//! stray scraper cannot turn the database counters into a denial of service.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;

use crate::auth::bearer_token;
use crate::error::ApiError;
use crate::metrics;
use crate::state::AppState;

/// Prometheus text format content type.
const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Renders the metrics registry and the current gauges.
pub async fn metrics(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    authorize(&state, &headers, peer)?;
    let snapshot = metrics::snapshot(&state).await;
    let mut body = state.metrics.render();
    body.push_str(&metrics::render_snapshot(&state, &snapshot));
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, CONTENT_TYPE)], body).into_response())
}

/// Enforces the bearer token, or the public rate limit.
pub fn authorize(state: &AppState, headers: &HeaderMap, peer: SocketAddr) -> Result<(), ApiError> {
    authorize_with(
        state.config.metrics_token.as_deref(),
        &state.metrics,
        headers,
        peer,
    )
}

fn authorize_with(
    expected: Option<&str>,
    registry: &metrics::Metrics,
    headers: &HeaderMap,
    peer: SocketAddr,
) -> Result<(), ApiError> {
    match expected {
        Some(expected) => {
            let provided = bearer_token(headers).ok_or_else(unauthorized)?;
            if token_matches(expected, &provided) {
                Ok(())
            } else {
                Err(unauthorized())
            }
        }
        None => {
            if registry.allow_scrape(peer.ip()) {
                Ok(())
            } else {
                Err(ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    "Too many metrics scrapes; retry in a minute",
                ))
            }
        }
    }
}

fn unauthorized() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "metrics_token_required",
        "A bearer token is required to scrape metrics",
    )
}

fn token_matches(expected: &str, provided: &str) -> bool {
    expected.len() == provided.len() && bool::from(expected.as_bytes().ct_eq(provided.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> SocketAddr {
        "127.0.0.1:9000".parse().expect("socket")
    }

    #[test]
    fn token_is_required_when_configured() {
        let registry = metrics::Metrics::default();
        let mut headers = HeaderMap::new();
        assert!(authorize_with(Some("secret-token"), &registry, &headers, peer()).is_err());
        headers.insert(header::AUTHORIZATION, "Bearer nope".parse().unwrap());
        assert!(authorize_with(Some("secret-token"), &registry, &headers, peer()).is_err());
        headers.insert(
            header::AUTHORIZATION,
            "Bearer secret-token".parse().unwrap(),
        );
        assert!(authorize_with(Some("secret-token"), &registry, &headers, peer()).is_ok());
    }

    #[test]
    fn public_scrapes_are_rate_limited() {
        let registry = metrics::Metrics::default();
        for _ in 0..30 {
            assert!(authorize_with(None, &registry, &HeaderMap::new(), peer()).is_ok());
        }
        assert!(authorize_with(None, &registry, &HeaderMap::new(), peer()).is_err());
        let other: SocketAddr = "127.0.0.2:9000".parse().expect("socket");
        assert!(authorize_with(None, &registry, &HeaderMap::new(), other).is_ok());
    }
}
