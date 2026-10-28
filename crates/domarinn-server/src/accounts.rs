//! HTTP handlers for local accounts, sessions, and API keys.
//!
//! These sit alongside the run/cache API in [`crate::routes`] and share its
//! [`ApiError`] type and the auth middleware. Open endpoints (`setup`, `login`,
//! `me`) inspect the request [`Identity`] directly; the management endpoints use
//! the [`Scoped`] extractors so they obey the active [`crate::AuthMode`].

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde::Deserialize;
use tokio::sync::Semaphore;
use ts_rs::TS;

use crate::auth::{self, Admin, Identity, Read, Scope, Scoped};
use crate::domain::{ApiKeyId, Role, UserId};
use crate::dto::accounts::{
    ApiKeyCreatedResponse, ApiKeyListResponse, ApiKeyView, AuthSessionResponse, MeResponse, MeUser,
    OkResponse, UserIdentityView, UserListResponse, UserView,
};
use crate::extract::ApiJson;
use crate::routes::{not_found, ApiError, ApiResult};
use crate::storage::{DeleteUserOutcome, UpdateUserOutcome};
use crate::AppState;

/// Minimum acceptable password length.
const MIN_PASSWORD_LEN: usize = 8;

// ---------------------------------------------------------------------------
// Failed-password throttling
// ---------------------------------------------------------------------------

/// Password attempts allowed per key per [`THROTTLE_WINDOW`].
const MAX_PASSWORD_ATTEMPTS: u32 = 5;
/// Failed login attempts allowed per peer address per [`THROTTLE_WINDOW`],
/// across every username. Higher than the per-username budget so a NAT full
/// of colleagues fumbling passwords is not locked out together, but low
/// enough that spraying random usernames (each a fresh per-username bucket)
/// cannot buy unbounded argon2 work.
const MAX_PEER_FAILURES: u32 = 30;
/// How long a key's attempts are remembered, measured from its first one.
const THROTTLE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Cap on tracked keys, so a spray of random usernames cannot grow the map
/// without bound.
const MAX_THROTTLE_KEYS: usize = 10_000;

struct Attempts {
    count: u32,
    window_start: Instant,
}

/// One bounded map of fixed-window attempt counters.
struct Buckets {
    limit: u32,
    entries: Mutex<HashMap<String, Attempts>>,
}

impl Buckets {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            entries: Mutex::default(),
        }
    }

    fn begin(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX_THROTTLE_KEYS && !entries.contains_key(key) {
            entries.retain(|_, a| now.saturating_duration_since(a.window_start) < THROTTLE_WINDOW);
            if entries.len() >= MAX_THROTTLE_KEYS {
                // Still full of live windows: drop the oldest, which is the
                // closest to expiring anyway.
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, a)| a.window_start)
                    .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest);
                }
            }
        }
        let attempts = entries.entry(key.to_string()).or_insert(Attempts {
            count: 0,
            window_start: now,
        });
        let elapsed = now.saturating_duration_since(attempts.window_start);
        if elapsed >= THROTTLE_WINDOW {
            attempts.count = 0;
            attempts.window_start = now;
        }
        if attempts.count >= self.limit {
            let remaining = THROTTLE_WINDOW.saturating_sub(elapsed);
            // Round up so a client that honors `Retry-After` always gets in.
            return Err(Duration::from_secs(
                remaining.as_secs_f64().ceil().max(1.0) as u64
            ));
        }
        attempts.count += 1;
        Ok(())
    }

    fn clear(&self, key: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    /// Give back one attempt that turned out not to be a failure.
    fn refund(&self, key: &str) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(attempts) = entries.get_mut(key) {
            attempts.count = attempts.count.saturating_sub(1);
        }
    }
}

/// In-memory, per-process backoff for password checks, in two layers:
///
/// - per key (an account, or a peer + username for login): at most
///   [`MAX_PASSWORD_ATTEMPTS`] unsuccessful attempts per [`THROTTLE_WINDOW`];
///   a success clears the key.
/// - per peer address, login only: at most [`MAX_PEER_FAILURES`] failures per
///   window across all usernames, checked first. A success refunds its own
///   attempt but never erases earlier failures, so holding one valid account
///   does not launder a spray against others.
///
/// Either layer answers a 429 with `Retry-After` until its window lapses.
/// Attempts are counted *before* the password is checked, so N concurrent
/// guesses cannot all slip past a check that has not yet seen any of them
/// fail; the successful path then clears (or refunds) them.
///
/// Per-process, like the MCP rate limiter: several replicas behind one load
/// balancer each allow their own budget. That still turns an online guessing
/// attack from unbounded into a handful of tries per quarter hour.
pub(crate) struct LoginThrottle {
    keys: Buckets,
    peers: Buckets,
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self {
            keys: Buckets::new(MAX_PASSWORD_ATTEMPTS),
            peers: Buckets::new(MAX_PEER_FAILURES),
        }
    }
}

impl LoginThrottle {
    /// Record an attempt for `key`, or `Err(retry_after)` when it is out of
    /// attempts for the current window.
    pub(crate) fn begin(&self, key: &str, now: Instant) -> Result<(), Duration> {
        self.keys.begin(key, now)
    }

    /// Forget `key` after a successful password check.
    pub(crate) fn clear(&self, key: &str) {
        self.keys.clear(key);
    }

    /// Record a login attempt from `peer`, or `Err(retry_after)` when that
    /// address is out of failures for the current window.
    pub(crate) fn begin_peer(&self, peer: IpAddr, now: Instant) -> Result<(), Duration> {
        self.peers.begin(&peer_bucket(peer), now)
    }

    /// Take back an attempt [`Self::begin_peer`] counted that did not end in
    /// a wrong password (it succeeded, or a later check refused it unverified).
    pub(crate) fn refund_peer(&self, peer: IpAddr) {
        self.peers.refund(&peer_bucket(peer));
    }
}

/// The per-peer bucket for an address. IPv6 clients are bucketed by their
/// /64, which is what one host or one customer is normally handed, so cycling
/// through the interface identifier does not mint fresh budgets.
fn peer_bucket(peer: IpAddr) -> String {
    match peer.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// The throttle key for a login attempt.
///
/// The peer address comes from axum's `ConnectInfo`, which is only present
/// when the server is served with `into_make_service_with_connect_info`; when
/// it is absent (and under the `oneshot` test harness) the key degrades to the
/// username alone. Forwarding headers are deliberately ignored: the server has
/// no trusted-proxy setting, so `X-Forwarded-For` is whatever the attacker
/// says it is, and honoring it would hand every guess a fresh bucket.
fn login_throttle_key(peer: Option<SocketAddr>, username: &str) -> String {
    match peer {
        Some(addr) => format!("login\0{}\0{username}", addr.ip()),
        None => format!("login\0-\0{username}"),
    }
}

/// A 429 carrying `Retry-After` in whole seconds.
fn throttled_response(retry_after: Duration) -> Response {
    let mut response = ApiError::status(
        StatusCode::TOO_MANY_REQUESTS,
        format!(
            "too many failed password attempts; try again in {} seconds",
            retry_after.as_secs()
        ),
    )
    .into_response();
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        HeaderValue::from(retry_after.as_secs()),
    );
    response
}

/// A real argon2 hash (default parameters, so the same cost as any stored
/// one) of a password nobody holds, verified against when the username has no
/// usable hash. Computed once, on first use.
fn dummy_hash() -> &'static str {
    static DUMMY: LazyLock<String> = LazyLock::new(|| {
        auth::hash_password(&auth::generate_session_token())
            .expect("hashing the timing-equalizer password")
    });
    DUMMY.as_str()
}

/// Bounds how many argon2 computations run at once. Each one costs ~19 MiB
/// and a core for tens of milliseconds; unbounded, a burst of password
/// requests would exhaust memory and the blocking pool long before the
/// throttle's budgets ran out across many keys.
fn hash_permits() -> &'static Semaphore {
    static PERMITS: LazyLock<Semaphore> = LazyLock::new(|| {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        Semaphore::new(cores.max(2))
    });
    &PERMITS
}

/// Run an argon2 computation off the async workers, behind [`hash_permits`],
/// so password checks never stall unrelated requests such as `/health`.
async fn run_argon2<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    let _permit = hash_permits()
        .acquire()
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!("password hashing unavailable: {e}")))?;
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!("password hashing task failed: {e}")))
}

/// [`auth::hash_password`] on the blocking pool.
async fn hash_password(password: &str) -> ApiResult<String> {
    let password = password.to_string();
    Ok(run_argon2(move || auth::hash_password(&password)).await??)
}

/// [`auth::verify_password`] on the blocking pool; `None` verifies against
/// [`dummy_hash`] so a missing hash costs the same as a real one.
async fn verify_password(stored: Option<String>, password: &str) -> ApiResult<bool> {
    let password = password.to_string();
    run_argon2(move || {
        let stored = stored.as_deref().unwrap_or_else(|| dummy_hash());
        auth::verify_password(stored, &password)
    })
    .await
}

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
    let hash = hash_password(&body.password).await?;
    let user = state
        .storage
        .create_user(username, hash, Role::Admin)
        .await?
        .ok_or_else(|| ApiError::status(StatusCode::CONFLICT, "username already exists"))?;
    let token = issue_session(&state, &user.id).await?;
    Ok(session_response(&state, StatusCode::CREATED, token, &user))
}

/// `POST /auth/login` — exchange username + password for a session token.
///
/// Throttled per peer address and then per (peer address, username) by
/// [`LoginThrottle`], and constant-shape for every failure: an unknown username, an SSO-only
/// account, and a disabled account all pay for one argon2 verify (against
/// [`dummy_hash`] when there is no real one) and get the same 401, so neither
/// the status nor the latency tells a caller which usernames exist.
pub(crate) async fn login(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    ApiJson(body): ApiJson<CredentialsBody>,
) -> ApiResult<Response> {
    let peer = peer.map(|Extension(ConnectInfo(addr))| addr);
    let now = Instant::now();
    if let Some(addr) = peer {
        if let Err(retry_after) = state.login_throttle.begin_peer(addr.ip(), now) {
            return Ok(throttled_response(retry_after));
        }
    }
    let key = login_throttle_key(peer, &body.username);
    if let Err(retry_after) = state.login_throttle.begin(&key, now) {
        // Refused before any password was checked: not a peer failure.
        if let Some(addr) = peer {
            state.login_throttle.refund_peer(addr.ip());
        }
        return Ok(throttled_response(retry_after));
    }
    let user = state
        .storage
        .get_user_by_username(body.username.clone())
        .await?;
    // SSO-only accounts carry the empty-string password sentinel, which
    // `verify_password` would reject instantly; they verify against the dummy
    // hash instead, like an unknown username, so they cost the same.
    let stored = user
        .as_ref()
        .map(|u| u.password_hash.clone())
        .filter(|h| !h.is_empty());
    let has_password = stored.is_some();
    let password_ok = verify_password(stored, &body.password).await?;
    let user = match user {
        Some(user) if has_password && password_ok && !user.disabled => user,
        _ => {
            return Err(ApiError::status(
                StatusCode::UNAUTHORIZED,
                "invalid credentials",
            ))
        }
    };
    state.login_throttle.clear(&key);
    if let Some(addr) = peer {
        state.login_throttle.refund_peer(addr.ip());
    }
    let token = issue_session(&state, &user.id).await?;
    Ok(session_response(&state, StatusCode::OK, token, &user))
}

#[derive(Debug, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangePasswordBody {
    current_password: String,
    new_password: String,
}

/// `POST /me/password` — change the caller's own password.
///
/// Requires the current password even though the caller is already
/// authenticated: a borrowed laptop or a stolen session cookie must not be
/// enough to take the account over for good. Every *other* session of the
/// user is revoked; the presenting one survives, so the browser that made the
/// change stays signed in. Wrong guesses at `current_password` count against
/// the same [`LoginThrottle`] as login, keyed on the account.
pub(crate) async fn change_password(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ChangePasswordBody>,
) -> ApiResult<Response> {
    // `Read` is open to anonymous callers outside `closed` mode; there is no
    // "own password" to change without a credential, so say so with a 401
    // (as `logout` does) rather than `require_user`'s 403.
    if !scope.identity.is_authenticated() {
        return Err(ApiError::status(
            StatusCode::UNAUTHORIZED,
            "authentication required",
        ));
    }
    let user_id = require_user(&scope.identity)?;
    let user = state
        .storage
        .get_user_by_id(user_id.clone())
        .await?
        .ok_or_else(|| not_found("user"))?;
    if user.password_hash.is_empty() {
        return Err(ApiError::status(
            StatusCode::BAD_REQUEST,
            "this account signs in through SSO and has no local password to change",
        ));
    }
    validate_password(&body.new_password)?;

    let key = format!("password-change\0{}", user_id.as_str());
    if let Err(retry_after) = state.login_throttle.begin(&key, Instant::now()) {
        return Ok(throttled_response(retry_after));
    }
    // 403, not 401: the caller *is* authenticated, and the web client treats
    // any 401 as "your session is gone" and prompts for a new credential.
    if !verify_password(Some(user.password_hash.clone()), &body.current_password).await? {
        return Err(ApiError::status(
            StatusCode::FORBIDDEN,
            "current password is incorrect",
        ));
    }
    state.login_throttle.clear(&key);

    let hash = hash_password(&body.new_password).await?;
    state
        .storage
        .update_password_keeping_session(user_id, hash, scope.identity.session_token_hash.clone())
        .await?;
    Ok(Json(OkResponse { ok: true }).into_response())
}

/// `POST /auth/logout` — revoke the presenting session and clear the session
/// cookie. A no-op (but still 200) for API-key or static-token callers.
/// Anonymous callers get a 401.
pub(crate) async fn logout(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> ApiResult<Response> {
    if !identity.is_authenticated() {
        return Err(ApiError::status(
            StatusCode::UNAUTHORIZED,
            "authentication required",
        ));
    }
    if let Some(hash) = identity.session_token_hash {
        state.storage.delete_session(hash).await?;
    }
    let mut response = Json(OkResponse { ok: true }).into_response();
    response.headers_mut().append(
        axum::http::header::SET_COOKIE,
        auth::clear_session_cookie(state.cookie_secure),
    );
    Ok(response)
}

/// `GET /auth/me` — report the current identity (or anonymity).
pub(crate) async fn me(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> ApiResult<Json<MeResponse>> {
    let user = match (&identity.user_id, &identity.username, &identity.role) {
        (Some(id), Some(username), Some(role)) => {
            let identities = state.storage.list_identities_for_user(id.clone()).await?;
            let views = identities.iter().map(UserIdentityView::from).collect();
            let role_managed_by = identities.first().map(|i| i.provider.clone());
            Some(MeUser {
                id: id.clone(),
                username: username.clone(),
                role: *role,
                identities: views,
                role_managed_by,
            })
        }
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
///
/// Gated at `read`, not `write`, and the three key endpoints below agree.
/// What protects them is not the route gate but the two per-request checks:
/// [`require_user`], which refuses any credential with no owning account
/// (a static token can never mint a key), and the scope ceiling in
/// [`create_apikey`], which caps a minted key at the caller's own scope. A
/// `write` gate on top of those bought nothing and cost something real — in
/// `closed` mode it locked a viewer out of minting the read-only key their
/// role is entirely about.
pub(crate) async fn list_apikeys(
    scope: Scoped<Read>,
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
    scope: Scoped<Read>,
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
    scope: Scoped<Read>,
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

/// `GET /users` — list all accounts (admin only), each with its linked SSO
/// identities (one `list_all_identities` fetch, grouped by user).
pub(crate) async fn list_users(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
) -> ApiResult<Json<UserListResponse>> {
    let users = state.storage.list_users().await?;
    let all_identities = state.storage.list_all_identities().await?;
    let mut by_user: std::collections::HashMap<UserId, Vec<_>> = std::collections::HashMap::new();
    for identity in all_identities {
        by_user
            .entry(identity.user_id.clone())
            .or_default()
            .push(identity);
    }
    let users = users
        .iter()
        .map(|user| UserView::from_row(user, by_user.get(&user.id).map_or(&[], |v| v.as_slice())))
        .collect();
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
    let hash = hash_password(&body.password).await?;
    let user = state
        .storage
        .create_user(username, hash, body.role)
        .await?
        .ok_or_else(|| ApiError::status(StatusCode::CONFLICT, "username already exists"))?;
    Ok((StatusCode::CREATED, Json(UserView::from(&user))).into_response())
}

/// `PATCH /users/{id}` — change role, enabled state, and/or password.
///
/// A password reset revokes all of the target's sessions, and so does
/// disabling the account (in storage, alongside the flag). An admin resetting
/// their *own* password keeps the session they made the request with.
pub(crate) async fn patch_user(
    scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path(id): Path<UserId>,
    ApiJson(body): ApiJson<PatchUserBody>,
) -> ApiResult<Json<UserView>> {
    // Confirm the target exists before applying any partial update.
    if state.storage.get_user_by_id(id.clone()).await?.is_none() {
        return Err(not_found("user"));
    }
    // Role/disabled changes go through the guarded, transactional path so a
    // demotion or a disable can never remove the instance's last enabled admin
    // (the same invariant `delete_user` enforces).
    if body.role.is_some() || body.disabled.is_some() {
        match state
            .storage
            .update_role_and_disabled(id.clone(), body.role, body.disabled)
            .await?
        {
            UpdateUserOutcome::Updated => {}
            UpdateUserOutcome::NotFound => return Err(not_found("user")),
            UpdateUserOutcome::LastAdmin => {
                return Err(ApiError::status(
                    StatusCode::CONFLICT,
                    "cannot remove the last admin",
                ))
            }
        }
    }
    if let Some(password) = &body.password {
        validate_password(password)?;
        let hash = hash_password(password).await?;
        let keep = if scope.identity.user_id.as_ref() == Some(&id) {
            scope.identity.session_token_hash.clone()
        } else {
            None
        };
        state
            .storage
            .update_password_keeping_session(id.clone(), hash, keep)
            .await?;
    }
    let updated = state
        .storage
        .get_user_by_id(id.clone())
        .await?
        .ok_or_else(|| not_found("user"))?;
    let identities = state.storage.list_identities_for_user(id).await?;
    Ok(Json(UserView::from_row(&updated, &identities)))
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
pub(crate) async fn issue_session(state: &AppState, user_id: &UserId) -> ApiResult<String> {
    let token = auth::generate_session_token();
    let hash = auth::token_hash(&token);
    let expires_at = auth::session_expiry(Utc::now().timestamp_millis());
    state
        .storage
        .create_session(hash, user_id.clone(), expires_at)
        .await?;
    Ok(token)
}

/// A login/setup success: the session token in the JSON body (scripting
/// consumers) *and* as the `Set-Cookie` session cookie (browsers, SSO).
fn session_response(
    state: &AppState,
    status: StatusCode,
    token: String,
    user: &crate::storage::UserRow,
) -> Response {
    let cookie = auth::session_cookie(&token, state.cookie_secure);
    let mut response = (
        status,
        Json(AuthSessionResponse {
            token,
            user: UserView::from(user),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .append(axum::http::header::SET_COOKIE, cookie);
    response
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sixth_attempt_in_a_window_is_refused_with_the_remaining_wait() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for i in 0..MAX_PASSWORD_ATTEMPTS {
            assert!(throttle.begin("k", start).is_ok(), "attempt {i}");
        }
        let later = start + Duration::from_secs(60);
        let wait = throttle.begin("k", later).unwrap_err();
        assert_eq!(wait, THROTTLE_WINDOW - Duration::from_secs(60));
    }

    #[test]
    fn the_window_lapsing_restores_the_budget() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            throttle.begin("k", start).unwrap();
        }
        assert!(throttle.begin("k", start).is_err());
        assert!(throttle.begin("k", start + THROTTLE_WINDOW).is_ok());
    }

    #[test]
    fn a_success_clears_the_key_and_keys_are_independent() {
        let throttle = LoginThrottle::default();
        let now = Instant::now();
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            throttle.begin("a", now).unwrap();
        }
        assert!(throttle.begin("a", now).is_err());
        assert!(throttle.begin("b", now).is_ok());
        throttle.clear("a");
        assert!(throttle.begin("a", now).is_ok());
    }

    #[test]
    fn retry_after_rounds_up_and_is_never_zero() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            throttle.begin("k", start).unwrap();
        }
        let nearly = start + THROTTLE_WINDOW - Duration::from_millis(1);
        assert_eq!(
            throttle.begin("k", nearly).unwrap_err(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn the_map_is_bounded() {
        let throttle = LoginThrottle::default();
        let now = Instant::now();
        for i in 0..MAX_THROTTLE_KEYS + 50 {
            throttle.begin(&format!("k{i}"), now).unwrap();
        }
        assert_eq!(
            throttle.keys.entries.lock().unwrap().len(),
            MAX_THROTTLE_KEYS
        );
    }

    #[test]
    fn the_key_ignores_the_port_and_degrades_to_username_without_a_peer() {
        let a: SocketAddr = "10.0.0.1:4000".parse().unwrap();
        let b: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        assert_eq!(
            login_throttle_key(Some(a), "root"),
            login_throttle_key(Some(b), "root")
        );
        assert_ne!(
            login_throttle_key(Some(a), "root"),
            login_throttle_key(None, "root")
        );
        assert_ne!(
            login_throttle_key(None, "root"),
            login_throttle_key(None, "rooT")
        );
    }

    #[test]
    fn the_peer_bucket_trips_across_usernames_and_successes_only_refund() {
        let throttle = LoginThrottle::default();
        let now = Instant::now();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for i in 0..MAX_PEER_FAILURES {
            throttle.begin(&format!("user{i}"), now).unwrap();
            assert!(throttle.begin_peer(ip, now).is_ok(), "attempt {i}");
        }
        assert!(throttle.begin_peer(ip, now).is_err());
        assert!(throttle
            .begin_peer("10.0.0.2".parse().unwrap(), now)
            .is_ok());
        // A refund gives back exactly one attempt, not the whole budget.
        throttle.refund_peer(ip);
        assert!(throttle.begin_peer(ip, now).is_ok());
        assert!(throttle.begin_peer(ip, now).is_err());
        assert!(throttle.begin_peer(ip, now + THROTTLE_WINDOW).is_ok());
    }

    #[test]
    fn ipv6_peers_share_a_bucket_per_64_and_mapped_v4_is_v4() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(peer_bucket(a), peer_bucket(b));
        assert_ne!(peer_bucket(a), peer_bucket(c));
        let mapped: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
        assert_eq!(peer_bucket(mapped), "10.0.0.1");
    }

    #[tokio::test]
    async fn argon2_runs_off_the_async_workers() {
        let hash = hash_password("correct horse").await.ok().unwrap();
        assert!(verify_password(Some(hash), "correct horse")
            .await
            .ok()
            .unwrap());
        assert!(!verify_password(None, "").await.ok().unwrap());
    }

    #[test]
    fn the_dummy_hash_is_a_real_argon2_hash() {
        assert!(dummy_hash().starts_with("$argon2"));
        assert!(!auth::verify_password(dummy_hash(), ""));
    }
}
