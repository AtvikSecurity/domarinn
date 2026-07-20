//! HTTP handlers and the API router.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};

use measurellm_core::cache::CacheKey;
use measurellm_core::result::{CaseStatus, RunResult, RESULT_SCHEMA_VERSION};

use crate::auth::{Admin, Read, Scoped, Write};
use crate::domain::RunStatusFilter;
use crate::extract::{ApiJson, ApiQuery};
use crate::storage::{self, CachePutOutcome, CaseListFilter, IngestOutcome, RunListFilter};
use crate::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const MAX_BODY: usize = 64 * 1024 * 1024;

/// Build the full API router with all middleware layers applied.
pub fn router(state: AppState) -> Router {
    use crate::accounts;
    let api = Router::new()
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .route("/api/v1/meta", get(meta))
        // Local accounts, sessions, and API keys.
        .route("/api/v1/auth/setup", post(accounts::setup))
        .route("/api/v1/auth/login", post(accounts::login))
        .route("/api/v1/auth/logout", post(accounts::logout))
        .route("/api/v1/auth/me", get(accounts::me))
        .route(
            "/api/v1/apikeys",
            get(accounts::list_apikeys).post(accounts::create_apikey),
        )
        .route("/api/v1/apikeys/{id}", delete(accounts::delete_apikey))
        .route(
            "/api/v1/users",
            get(accounts::list_users).post(accounts::create_user),
        )
        .route(
            "/api/v1/users/{id}",
            patch(accounts::patch_user).delete(accounts::delete_user),
        )
        .route("/api/v1/runs", get(list_runs).post(post_run))
        .route("/api/v1/runs/{id}", get(get_run).delete(delete_run))
        .route("/api/v1/runs/{id}/cases", get(list_cases))
        .route("/api/v1/runs/{id}/cases/{case_key}", get(get_case))
        .route("/api/v1/runs/{id}/export", get(export_run))
        .route("/api/v1/runs/{id}/compare/{other}", get(compare_runs))
        .route("/api/v1/projects", get(list_projects))
        .route("/api/v1/projects/{project}/suites", get(list_suites))
        .route(
            "/api/v1/projects/{project}/suites/{suite}/baseline",
            put(put_baseline).delete(delete_baseline),
        )
        .route("/api/v1/cache/stats", get(cache_stats))
        .route("/api/v1/cache/prune", post(cache_prune))
        .route(
            "/api/v1/cache/{key}",
            get(cache_get).head(cache_head).put(cache_put),
        )
        .route("/assets/{*path}", get(serve_asset))
        .fallback(spa_fallback);

    api.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::auth_middleware,
    ))
    .layer(tower_http::decompression::RequestDecompressionLayer::new())
    .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
    .layer(tower_http::trace::TraceLayer::new_for_http())
    .with_state(state)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Handler error that renders as a JSON `{ "error": ... }` body.
pub enum ApiError {
    Status(StatusCode, String),
    Internal(anyhow::Error),
}

impl ApiError {
    pub(crate) fn status(code: StatusCode, msg: impl Into<String>) -> Self {
        ApiError::Status(code, msg.into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Status(code, msg) => (code, msg),
            ApiError::Internal(e) => {
                tracing::error!(error = %e, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

// ---------------------------------------------------------------------------
// Open endpoints
// ---------------------------------------------------------------------------

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn meta(State(state): State<AppState>) -> ApiResult<Response> {
    let current = RESULT_SCHEMA_VERSION;
    let min = current.saturating_sub(1);
    let supported: Vec<u32> = (min..=current).collect();
    let setup_required = state.storage.count_users().await? == 0;
    Ok(Json(json!({
        "name": "measurellm",
        "version": measurellm_core::VERSION,
        "auth_mode": state.auth_mode,
        "setup_required": setup_required,
        "supported_schema_versions": supported,
        "result_schema_version": current,
        "cache": {
            "max_entry_bytes": state.cache_limits.max_entry_bytes,
            "max_bytes": state.cache_limits.max_bytes,
            "max_age_days": state.cache_limits.max_age_days,
        },
    }))
    .into_response())
}

// ---------------------------------------------------------------------------
// Embedded web UI
// ---------------------------------------------------------------------------

/// The built React/Vite UI, embedded from `web/dist`. In debug builds
/// `rust-embed` reads these files from disk at runtime; release builds embed
/// them into the binary. The folder path is relative to this crate's
/// `Cargo.toml`.
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// Vite fingerprints every file under `/assets`, so those may be cached
/// forever.
const ASSET_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";
/// The SPA shell (and un-hashed top-level files) must be revalidated so a new
/// deploy is picked up.
const SHELL_CACHE_CONTROL: &str = "no-cache";

/// Shown only when no `index.html` is embedded at all (e.g. the UI was never
/// built). The real embedded `index.html` is always preferred when present.
const UNBUILT_HTML: &str =
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>measurellm</title></head>\
     <body style=\"font-family:system-ui;max-width:40rem;margin:4rem auto;padding:0 1rem\">\
     <h1>measurellm</h1><p>The results UI is not built into this binary yet. \
     The JSON API is available under <code>/api/v1</code>.</p></body></html>";

/// Build a response for the embedded file at `path` (relative to `web/dist`),
/// deriving `Content-Type` from its extension and applying `cache_control`.
/// Returns `None` when no such file is embedded.
fn embedded_file(path: &str, cache_control: &'static str) -> Option<Response> {
    let file = WebAssets::get(path)?;
    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let headers = [
        (header::CONTENT_TYPE, content_type),
        (header::CACHE_CONTROL, cache_control.to_string()),
    ];
    Some((headers, file.data.into_owned()).into_response())
}

/// Serve the SPA shell (`index.html`) with a revalidating cache header, falling
/// back to a built-in "UI not built" page when nothing is embedded.
fn serve_shell() -> Response {
    embedded_file("index.html", SHELL_CACHE_CONTROL)
        .unwrap_or_else(|| Html(UNBUILT_HTML).into_response())
}

/// `GET /assets/{*path}` — hashed, immutable static assets emitted by Vite. A
/// miss returns a JSON 404 rather than the SPA shell.
async fn serve_asset(Path(path): Path<String>) -> Response {
    match embedded_file(&format!("assets/{path}"), ASSET_CACHE_CONTROL) {
        Some(resp) => resp,
        None => not_found("asset").into_response(),
    }
}

/// Router fallback. Requests under `/api/` that matched no route stay JSON 404
/// (never the SPA). Any other path serves a top-level embedded file when one
/// exists (e.g. `/favicon.ico`, `/vite.svg`), otherwise the SPA shell so that
/// client-side routes resolve to the app.
async fn spa_fallback(uri: Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") {
        return not_found("route").into_response();
    }
    let rel = path.trim_start_matches('/');
    if !rel.is_empty() {
        if let Some(resp) = embedded_file(rel, SHELL_CACHE_CONTROL) {
            return resp;
        }
    }
    serve_shell()
}

// ---------------------------------------------------------------------------
// Run ingest
// ---------------------------------------------------------------------------

async fn post_run(
    scope: Scoped<Write>,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let value: Value = serde_json::from_slice(&body).map_err(|e| {
        ApiError::status(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}"))
    })?;

    let schema_version = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::status(StatusCode::BAD_REQUEST, "missing schema_version"))?;
    let current = RESULT_SCHEMA_VERSION as u64;
    let min = current.saturating_sub(1);
    if schema_version < min || schema_version > current {
        return Err(ApiError::status(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unsupported schema_version {schema_version}; supported: {min}..={current}"),
        ));
    }

    let run: RunResult = serde_json::from_value(value).map_err(|e| {
        ApiError::status(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("invalid run document: {e}"),
        )
    })?;
    let run_id = run.run_id.clone();
    let uploaded_by = scope.identity.label.clone();

    let outcome = state.storage.ingest_run(run, uploaded_by).await?;
    let url = build_run_url(&state, &headers, &run_id);

    match outcome {
        IngestOutcome::Created => Ok((
            StatusCode::CREATED,
            Json(json!({ "id": run_id, "url": url })),
        )
            .into_response()),
        IngestOutcome::Existing => {
            Ok((StatusCode::OK, Json(json!({ "id": run_id, "url": url }))).into_response())
        }
        IngestOutcome::Conflict => Ok((
            StatusCode::CONFLICT,
            Json(json!({
                "error": "a different run already exists with this id",
                "id": run_id,
            })),
        )
            .into_response()),
    }
}

fn build_run_url(state: &AppState, headers: &HeaderMap, run_id: &str) -> String {
    let base = if let Some(public) = &state.public_url {
        public.trim_end_matches('/').to_string()
    } else {
        let host = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost");
        let scheme = headers
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("http");
        format!("{scheme}://{host}")
    };
    format!("{base}/runs/{run_id}")
}

// ---------------------------------------------------------------------------
// Run reads
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RunQuery {
    project: Option<String>,
    suite: Option<String>,
    tag: Option<String>,
    branch: Option<String>,
    since: Option<String>,
    until: Option<String>,
    status: Option<RunStatusFilter>,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_runs(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<RunQuery>,
) -> ApiResult<Response> {
    let filter = RunListFilter {
        project: q.project,
        suite: q.suite,
        tag: q.tag,
        branch: q.branch,
        since_ms: q.since.as_deref().and_then(storage::parse_time_ms),
        until_ms: q.until.as_deref().and_then(storage::parse_time_ms),
        status: q.status,
        limit: clamp_limit(q.limit),
        cursor: q.cursor.as_deref().and_then(storage::decode_cursor),
    };
    let page = state.storage.list_runs(filter).await?;
    Ok(Json(json!({
        "runs": page.runs,
        "next_cursor": page.next_cursor,
    }))
    .into_response())
}

async fn get_run(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    match state.storage.get_run(id).await? {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Err(not_found("run")),
    }
}

#[derive(Debug, Deserialize)]
struct CaseQuery {
    status: Option<CaseStatus>,
    tag: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_cases(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<CaseQuery>,
) -> ApiResult<Response> {
    if !state.storage.run_exists(id.clone()).await? {
        return Err(not_found("run"));
    }
    let filter = CaseListFilter {
        run_id: id,
        status: q.status,
        tag: q.tag,
        q: q.q,
        limit: clamp_limit(q.limit),
        cursor: q.cursor.as_deref().and_then(|c| c.parse::<i64>().ok()),
    };
    let page = state.storage.list_cases(filter).await?;
    Ok(Json(page).into_response())
}

async fn get_case(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((id, case_key)): Path<(String, String)>,
) -> ApiResult<Response> {
    match state.storage.get_case(id, case_key).await? {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Err(not_found("case")),
    }
}

async fn export_run(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    match state.storage.export_run(id).await? {
        Some(doc) => Ok(Json(doc).into_response()),
        None => Err(not_found("run")),
    }
}

async fn compare_runs(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((id, other)): Path<(String, String)>,
) -> ApiResult<Response> {
    match state.storage.compare_runs(id, other).await? {
        Some(cmp) => Ok(Json(cmp).into_response()),
        None => Err(not_found("run")),
    }
}

async fn delete_run(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    if state.storage.delete_run(id).await? {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(not_found("run"))
    }
}

// ---------------------------------------------------------------------------
// Projects, suites, baselines
// ---------------------------------------------------------------------------

async fn list_projects(_scope: Scoped<Read>, State(state): State<AppState>) -> ApiResult<Response> {
    Ok(Json(state.storage.list_projects().await?).into_response())
}

async fn list_suites(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> ApiResult<Response> {
    Ok(Json(state.storage.list_suites(project).await?).into_response())
}

#[derive(Debug, Deserialize)]
struct BaselineBody {
    run_id: String,
}

async fn put_baseline(
    _scope: Scoped<Write>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
    ApiJson(body): ApiJson<BaselineBody>,
) -> ApiResult<Response> {
    if state
        .storage
        .set_baseline(project.clone(), suite.clone(), body.run_id.clone())
        .await?
    {
        Ok(Json(json!({
            "project": project,
            "suite": suite,
            "run_id": body.run_id,
        }))
        .into_response())
    } else {
        Err(not_found("run"))
    }
}

async fn delete_baseline(
    _scope: Scoped<Write>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
) -> ApiResult<Response> {
    if state.storage.delete_baseline(project, suite).await? {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(not_found("baseline"))
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

fn validate_cache_key(key: &str) -> ApiResult<()> {
    if CacheKey::is_valid(key) {
        Ok(())
    } else {
        Err(ApiError::status(
            StatusCode::BAD_REQUEST,
            "invalid cache key; expected sha256:<64 hex>",
        ))
    }
}

async fn cache_get(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Response> {
    validate_cache_key(&key)?;
    match state.storage.cache_get(key).await? {
        Some(bytes) => {
            Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
        }
        None => Err(not_found("cache entry")),
    }
}

async fn cache_head(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Response> {
    validate_cache_key(&key)?;
    if state.storage.cache_has(key).await? {
        Ok(StatusCode::OK.into_response())
    } else {
        Ok(StatusCode::NOT_FOUND.into_response())
    }
}

async fn cache_put(
    _scope: Scoped<Write>,
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    validate_cache_key(&key)?;
    if body.len() > state.cache_limits.max_entry_bytes {
        return Err(ApiError::status(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "entry exceeds max size of {} bytes",
                state.cache_limits.max_entry_bytes
            ),
        ));
    }
    match state.storage.cache_put(key, body.to_vec()).await? {
        CachePutOutcome::Created => Ok(StatusCode::CREATED.into_response()),
        CachePutOutcome::Exists => Ok(StatusCode::OK.into_response()),
    }
}

async fn cache_stats(_scope: Scoped<Read>, State(state): State<AppState>) -> ApiResult<Response> {
    Ok(Json(state.storage.cache_stats().await?).into_response())
}

#[derive(Debug, Deserialize)]
struct PruneQuery {
    older_than_days: Option<i64>,
    target_bytes: Option<i64>,
}

async fn cache_prune(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<PruneQuery>,
) -> ApiResult<Response> {
    let pruned = state
        .storage
        .cache_prune(q.older_than_days, q.target_bytes)
        .await?;
    Ok(Json(json!({ "pruned": pruned })).into_response())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn clamp_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

pub(crate) fn not_found(what: &str) -> ApiError {
    ApiError::status(StatusCode::NOT_FOUND, format!("{what} not found"))
}
