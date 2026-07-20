//! HTTP handlers for local accounts, sessions, and API keys.
//!
//! These sit alongside the run/cache API in [`crate::routes`] and share its
//! [`ApiError`] type and the auth middleware. Open endpoints (`setup`, `login`,
//! `me`) inspect the request [`Identity`] directly; the management endpoints use
//! the [`Scoped`] extractors so they obey the active [`crate::AuthMode`].

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde::Deserialize;
use ts_rs::TS;

use crate::auth::{self, Admin, Identity, Scope, Scoped, Write};
use crate::domain::{ApiKeyId, Role, UserId};
use crate::dto::accounts::{
    ApiKeyCreatedResponse, ApiKeyListResponse, ApiKeyView, AuthSessionResponse, MeResponse, MeUser,
    OkResponse, UserListResponse, UserView,
};
use crate::extract::ApiJson;
use crate::routes::{not_found, ApiError, ApiResult};
use crate::storage::DeleteUserOutcome;
use crate::AppState;

/// Minimum acceptable password length.
const MIN_PASSWORD_LEN: usize = 8;

// ---------------------------------------------------------------------------
// Setup / login / logout / me
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialsBody {
    username: String,
    password: String,
}

/// `POST /auth/setup` — create the first admin. Open, but only while zero users
/// exist; afterwards it is a 409.
pub(crate) async fn setup(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CredentialsBody>,
) -> ApiResult<Response> {
    if state.storage.count_users().await? > 0 {
        return Err(ApiError::status(
            StatusCode::CONFLICT,
            "setup has already been completed",
        ));
    }
    let username = validate_username(&body.username)?;
    validate_password(&body.password)?;
    let hash = auth::hash_password(&body.password)?;
    let user = state
        .storage
        .create_user(username, hash, Role::Admin)
        .await?
        .ok_or_else(|| ApiError::status(StatusCode::CONFLICT, "username already exists"))?;
    let token = issue_session(&state, &user.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(AuthSessionResponse {
            token,
            user: UserView::from(&user),
        }),
    )
        .into_response())
}

/// `POST /auth/login` — exchange username + password for a session token.
pub(crate) async fn login(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CredentialsBody>,
) -> ApiResult<Response> {
    let invalid = || ApiError::status(StatusCode::UNAUTHORIZED, "invalid credentials");
    let Some(user) = state
        .storage
        .get_user_by_username(body.username.clone())
        .await?
    else {
        return Err(invalid());
    };
    if user.disabled || !auth::verify_password(&user.password_hash, &body.password) {
        return Err(invalid());
    }
    let token = issue_session(&state, &user.id).await?;
    Ok((
        StatusCode::OK,
        Json(AuthSessionResponse {
            token,
            user: UserView::from(&user),
        }),
    )
        .into_response())
}

/// `POST /auth/logout` — revoke the presenting session. A no-op (but still 200)
/// for API-key or static-token callers. Anonymous callers get a 401.
pub(crate) async fn logout(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> ApiResult<Json<OkResponse>> {
    if !identity.is_authenticated() {
        return Err(ApiError::status(
            StatusCode::UNAUTHORIZED,
            "authentication required",
        ));
    }
    if let Some(hash) = identity.session_token_hash {
        state.storage.delete_session(hash).await?;
    }
    Ok(Json(OkResponse { ok: true }))
}

/// `GET /auth/me` — report the current identity (or anonymity).
pub(crate) async fn me(Extension(identity): Extension<Identity>) -> ApiResult<Json<MeResponse>> {
    let user = match (&identity.user_id, &identity.username, &identity.role) {
        (Some(id), Some(username), Some(role)) => Some(MeUser {
            id: id.clone(),
            username: username.clone(),
            role: *role,
        }),
        _ => None,
    };
    Ok(Json(MeResponse {
        authenticated: identity.is_authenticated(),
        user,
        source: identity.source,
        scope: identity.scope,
    }))
}

// ---------------------------------------------------------------------------
// API keys
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateKeyBody {
    name: Option<String>,
    scope: Option<Scope>,
}

/// `GET /apikeys` — the caller's own keys (never the secret).
pub(crate) async fn list_apikeys(
    scope: Scoped<Write>,
    State(state): State<AppState>,
) -> ApiResult<Json<ApiKeyListResponse>> {
    let user_id = require_user(&scope.identity)?;
    let keys = state.storage.list_api_keys(user_id).await?;
    let keys = keys.iter().map(ApiKeyView::from).collect();
    Ok(Json(ApiKeyListResponse { keys }))
}

/// `POST /apikeys` — mint a key, returning its secret exactly once. The scope
/// defaults to the caller's own and may never exceed it.
pub(crate) async fn create_apikey(
    scope: Scoped<Write>,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CreateKeyBody>,
) -> ApiResult<Response> {
    let user_id = require_user(&scope.identity)?;
    let ceiling = scope.identity.scope.unwrap_or(Scope::Read);
    let requested = body.scope.unwrap_or(ceiling);
    if requested > ceiling {
        return Err(ApiError::status(
            StatusCode::FORBIDDEN,
            "requested scope exceeds your own",
        ));
    }

    let secret = auth::generate_api_key();
    let prefix = auth::key_prefix(&secret);
    let hash = auth::token_hash(&secret);
    let info = state
        .storage
        .create_api_key(user_id, body.name.clone(), prefix, hash, requested)
        .await?;

    let response = ApiKeyCreatedResponse {
        key: secret,
        view: ApiKeyView::from(&info),
    };
    Ok((StatusCode::CREATED, Json(response)).into_response())
}

/// `DELETE /apikeys/{id}` — revoke a key. Allowed for its owner or any admin.
pub(crate) async fn delete_apikey(
    scope: Scoped<Write>,
    State(state): State<AppState>,
    Path(id): Path<ApiKeyId>,
) -> ApiResult<Response> {
    let requester = require_user(&scope.identity)?;
    let is_admin = scope.identity.role == Some(Role::Admin);
    let key = state
        .storage
        .get_api_key(id.clone())
        .await?
        .ok_or_else(|| not_found("api key"))?;
    if key.user_id != requester && !is_admin {
        return Err(ApiError::status(StatusCode::FORBIDDEN, "not your api key"));
    }
    state.storage.revoke_api_key(id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// User administration
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateUserBody {
    username: String,
    password: String,
    role: Role,
}

#[derive(Debug, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub(crate) struct PatchUserBody {
    role: Option<Role>,
    disabled: Option<bool>,
    password: Option<String>,
}

/// `GET /users` — list all accounts (admin only).
pub(crate) async fn list_users(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
) -> ApiResult<Json<UserListResponse>> {
    let users = state.storage.list_users().await?;
    let users = users.iter().map(UserView::from).collect();
    Ok(Json(UserListResponse { users }))
}

/// `POST /users` — create an account (admin only).
pub(crate) async fn create_user(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CreateUserBody>,
) -> ApiResult<Response> {
    let username = validate_username(&body.username)?;
    validate_password(&body.password)?;
    let hash = auth::hash_password(&body.password)?;
    let user = state
        .storage
        .create_user(username, hash, body.role)
        .await?
        .ok_or_else(|| ApiError::status(StatusCode::CONFLICT, "username already exists"))?;
    Ok((StatusCode::CREATED, Json(UserView::from(&user))).into_response())
}

/// `PATCH /users/{id}` — change role, enabled state, and/or password.
pub(crate) async fn patch_user(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path(id): Path<UserId>,
    ApiJson(body): ApiJson<PatchUserBody>,
) -> ApiResult<Json<UserView>> {
    // Confirm the target exists before applying any partial update.
    if state.storage.get_user_by_id(id.clone()).await?.is_none() {
        return Err(not_found("user"));
    }
    if let Some(role) = body.role {
        state.storage.set_user_role(id.clone(), role).await?;
    }
    if let Some(disabled) = body.disabled {
        state
            .storage
            .set_user_disabled(id.clone(), disabled)
            .await?;
    }
    if let Some(password) = &body.password {
        validate_password(password)?;
        let hash = auth::hash_password(password)?;
        state.storage.update_password(id.clone(), hash).await?;
    }
    let updated = state
        .storage
        .get_user_by_id(id)
        .await?
        .ok_or_else(|| not_found("user"))?;
    Ok(Json(UserView::from(&updated)))
}

/// `DELETE /users/{id}` — remove an account, refusing the last admin.
pub(crate) async fn delete_user(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path(id): Path<UserId>,
) -> ApiResult<Response> {
    match state.storage.delete_user(id).await? {
        DeleteUserOutcome::Deleted => Ok(StatusCode::NO_CONTENT.into_response()),
        DeleteUserOutcome::NotFound => Err(not_found("user")),
        DeleteUserOutcome::LastAdmin => Err(ApiError::status(
            StatusCode::CONFLICT,
            "cannot delete the last admin",
        )),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Mint a session for `user_id` and persist its hash; returns the raw token.
async fn issue_session(state: &AppState, user_id: &UserId) -> ApiResult<String> {
    let token = auth::generate_session_token();
    let hash = auth::token_hash(&token);
    let expires_at = auth::session_expiry(Utc::now().timestamp_millis());
    state
        .storage
        .create_session(hash, user_id.clone(), expires_at)
        .await?;
    Ok(token)
}

/// The user id behind the request, or a 403 when the credentials are not
/// account-backed (e.g. a static token has no owning user).
fn require_user(identity: &Identity) -> ApiResult<UserId> {
    identity.user_id.clone().ok_or_else(|| {
        ApiError::status(
            StatusCode::FORBIDDEN,
            "this endpoint requires a user account",
        )
    })
}

fn validate_username(username: &str) -> ApiResult<String> {
    let trimmed = username.trim();
    if trimmed.is_empty() {
        return Err(ApiError::status(
            StatusCode::BAD_REQUEST,
            "username must not be empty",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_password(password: &str) -> ApiResult<()> {
    if password.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::status(
            StatusCode::BAD_REQUEST,
            format!("password must be at least {MIN_PASSWORD_LEN} characters"),
        ));
    }
    Ok(())
}
