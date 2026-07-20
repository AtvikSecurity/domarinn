//! `ApiJson`/`ApiQuery` — thin wrappers over axum's `Json`/`Query` extractors
//! whose rejections render through [`crate::routes::ApiError`] instead of
//! axum's plain-text defaults, so every failure mode in the API responds with
//! the server's standard `{"error": message}` body.
//!
//! Status mapping for JSON bodies: syntactically invalid JSON, a missing/wrong
//! `Content-Type`, or a body-buffering failure all map to 400; a body that
//! parses as JSON but fails to match the target type (e.g. an invalid `role`
//! or `scope` string) maps to 422. Query strings only have one failure mode
//! (`FailedToDeserializeQueryString`, already 400 by default) so `ApiQuery`
//! always maps to 400.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;
use serde::de::DeserializeOwned;

use crate::routes::ApiError;

/// Drop-in replacement for [`axum::Json`] whose rejection is an [`ApiError`]
/// (`{"error": ...}`) rather than axum's plain-text body.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(json_rejection_to_api_error(rejection)),
        }
    }
}

/// Drop-in replacement for [`axum::extract::Query`] whose rejection is an
/// [`ApiError`] (`{"error": ...}`) rather than axum's plain-text body.
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(ApiQuery(value)),
            Err(rejection) => Err(query_rejection_to_api_error(rejection)),
        }
    }
}

/// `JsonDataError` (valid JSON, wrong shape/values) -> 422; every other
/// failure mode (syntax error, missing content-type, body-buffering error)
/// -> 400.
fn json_rejection_to_api_error(rejection: JsonRejection) -> ApiError {
    let message = rejection.body_text();
    match rejection {
        JsonRejection::JsonDataError(_) => {
            ApiError::status(StatusCode::UNPROCESSABLE_ENTITY, message)
        }
        _ => ApiError::status(StatusCode::BAD_REQUEST, message),
    }
}

fn query_rejection_to_api_error(rejection: QueryRejection) -> ApiError {
    ApiError::status(StatusCode::BAD_REQUEST, rejection.body_text())
}
