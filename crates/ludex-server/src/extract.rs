//! Extractor wrappers that render contract-shaped JSON errors instead of axum's
//! default plain-text rejections.

use axum::extract::{FromRequest, FromRequestParts, Multipart, Path, Query, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// Maximum size accepted by [`OptionalJsonBody`] (64 KiB).
///
/// Endpoints with optional JSON bodies (share-link passwords, VirusTotal test
/// keys) only carry small structured payloads; capping the read tightly bounds
/// allocation before deserialization.
const MAX_OPTIONAL_JSON_BYTES: usize = 64 * 1024;

/// JSON body extractor returning [`ApiError`] on malformed input.
pub struct JsonBody<T>(pub T);

impl<T, S> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let axum::Json(value) = axum::Json::<T>::from_request(request, state)
            .await
            .map_err(|rejection| ApiError::bad_request("invalid_body", rejection.body_text()))?;
        Ok(Self(value))
    }
}

/// Optional JSON body extractor returning [`ApiError`] on malformed or
/// oversized input.
///
/// An empty body yields `Ok(OptionalJsonBody(None))` regardless of
/// `Content-Type`, so callers that send a bare `POST` keep working. Non-empty
/// bodies are bounded to [`MAX_OPTIONAL_JSON_BYTES`] and deserialized through
/// [`axum::Json::from_bytes`].
pub struct OptionalJsonBody<T>(pub Option<T>);

impl<T, S> FromRequest<S> for OptionalJsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let bytes = axum::body::to_bytes(request.into_body(), MAX_OPTIONAL_JSON_BYTES)
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "payload_too_large",
                    "Request body exceeds the maximum allowed size",
                )
            })?;
        if bytes.is_empty() {
            return Ok(Self(None));
        }
        let axum::Json(value) = axum::Json::<T>::from_bytes(&bytes)
            .map_err(|rejection| ApiError::bad_request("invalid_body", rejection.body_text()))?;
        Ok(Self(Some(value)))
    }
}

/// Multipart body extractor returning [`ApiError`] on malformed input.
///
/// Used by the cloud save upload route, whose body limit is much larger than
/// the JSON limit. Body-limit rejections surface as `413 save_too_large`.
pub struct MultipartBody(pub Multipart);

impl<S> FromRequest<S> for MultipartBody
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let multipart = Multipart::from_request(request, state)
            .await
            .map_err(|rejection| {
                if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "save_too_large",
                        "The save archive exceeds the server limit",
                    )
                } else {
                    ApiError::bad_request("invalid_multipart", rejection.body_text())
                }
            })?;
        Ok(Self(multipart))
    }
}

/// Query string extractor returning [`ApiError`] on malformed input.
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| ApiError::bad_request("invalid_query", rejection.body_text()))?;
        Ok(Self(value))
    }
}

/// Path extractor returning [`ApiError`] on malformed input.
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(value) = Path::<T>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| ApiError::bad_request("invalid_path", rejection.body_text()))?;
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    #[serde(deny_unknown_fields)]
    struct Sample {
        value: Option<String>,
    }

    async fn extract_optional(body: Body) -> Result<Option<Sample>, ApiError> {
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .body(body)
            .expect("request");
        OptionalJsonBody::<Sample>::from_request(request, &())
            .await
            .map(|OptionalJsonBody(inner)| inner)
    }

    #[tokio::test]
    async fn optional_json_body_accepts_empty_and_valid_payloads() {
        let empty = extract_optional(Body::empty()).await.expect("empty");
        assert_eq!(empty, None);
        let valid = extract_optional(Body::from(r#"{"value":"ok"}"#))
            .await
            .expect("valid");
        assert_eq!(
            valid,
            Some(Sample {
                value: Some("ok".to_string()),
            })
        );
    }

    #[tokio::test]
    async fn optional_json_body_rejects_invalid_unknown_and_oversized_payloads() {
        let invalid = extract_optional(Body::from("not json"))
            .await
            .expect_err("invalid json");
        assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
        assert_eq!(invalid.code, "invalid_body");

        let unknown = extract_optional(Body::from(r#"{"extra":true}"#))
            .await
            .expect_err("unknown field");
        assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
        assert_eq!(unknown.code, "invalid_body");

        let oversized_body = vec![b'a'; MAX_OPTIONAL_JSON_BYTES + 1];
        let oversized = extract_optional(Body::from(oversized_body))
            .await
            .expect_err("oversized");
        assert_eq!(oversized.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(oversized.code, "payload_too_large");
    }
}
