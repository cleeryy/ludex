//! Extractor wrappers that render contract-shaped JSON errors instead of axum's
//! default plain-text rejections.

use axum::extract::{FromRequest, FromRequestParts, Multipart, Path, Query, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

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
