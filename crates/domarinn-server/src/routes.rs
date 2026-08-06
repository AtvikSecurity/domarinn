//! HTTP handlers and the API router.

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::request_id::{
    MakeRequestId, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::trace::{DefaultOnFailure, TraceLayer};
use tracing::{info_span, Level, Span};
use ts_rs::TS;

use domarinn_core::cache::CacheKey;
use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::{CaseStatus, RunResult, RESULT_SCHEMA_VERSION};

use crate::auth::{Admin, Read, Scoped, Write};
use crate::domain::{CachedFilter, OriginFilter, RunStatusFilter};
use crate::dto::meta::{CacheTierMeta, MetaCacheLimits, MetaResponse};
use crate::dto::runs::{IngestResponse, RunListResponse};
use crate::dto::search::SearchResponse;
use crate::extract::{ApiJson, ApiQuery};
use crate::runsets::{GrantLevel, RunVisibility};
use crate::storage::{
    self, CachePutOutcome, CaseListFilter, IngestOutcome, MatrixFilter, RunListFilter,
};
use crate::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
/// The case-history endpoint has its own tighter window than the run/case
/// lists: a timeline of the last few runs, defaulting to 20 and capped at 100.
const HISTORY_DEFAULT_LIMIT: i64 = 20;
const HISTORY_MAX_LIMIT: i64 = 100;
/// The matrix endpoint paginates test ROWS (columns never paginate), with a
/// wider window than the case list: defaults to 100 and caps at 500.
const MATRIX_DEFAULT_LIMIT: i64 = 100;
const MATRIX_MAX_LIMIT: i64 = 500;
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
        // SSO login flows (unauthenticated by design — they are the way in).
        .route(
            "/api/v1/auth/oidc/{provider}/start",
            get(crate::sso::handlers::oidc_start),
        )
        .route(
            "/api/v1/auth/oidc/{provider}/callback",
            get(crate::sso::handlers::oidc_callback),
        );
    #[cfg(feature = "saml")]
    let api = api
        .route(
            "/api/v1/auth/saml/{provider}/start",
            get(crate::sso::handlers::saml_start),
        )
        .route(
            "/api/v1/auth/saml/{provider}/acs",
            post(crate::sso::handlers::saml_acs),
        )
        .route(
            "/api/v1/auth/saml/{provider}/metadata",
            get(crate::sso::handlers::saml_metadata),
        );
    let api = api
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
        .route("/api/v1/runs/{id}/matrix", get(run_matrix))
        .route("/api/v1/runs/{id}/export", get(export_run))
        .route("/api/v1/runs/{id}/config", get(get_run_config))
        .route("/api/v1/runs/{id}/compare/{other}", get(compare_runs))
        .route("/api/v1/search", get(search))
        .route("/api/v1/projects", get(list_projects))
        .route("/api/v1/projects/{project}/suites", get(list_suites))
        .route(
            "/api/v1/projects/{project}/suites/{suite}/baseline",
            put(put_baseline).delete(delete_baseline),
        )
        .route(
            "/api/v1/projects/{project}/suites/{suite}/cases/{case_key}/history",
            get(case_history),
        )
        .route("/api/v1/cache/stats", get(crate::cachebrowse::cache_stats))
        .route("/api/v1/cache/prune", post(crate::cachebrowse::cache_prune))
        // Static segments win over `{key}` in axum's router, and more strongly:
        // `validate_cache_key` requires `sha256:<64 hex>`, so no legal key can
        // ever be the literal `entries` or `facets`. `/cache/stats` already
        // proves the pattern in production.
        .route("/api/v1/cache/entries", get(crate::cachebrowse::list))
        .route("/api/v1/cache/facets", get(crate::cachebrowse::facets))
        // One `MethodRouter` per path: axum takes a single one, so `delete`
        // chains onto `get` here rather than registering the path twice.
        .route(
            "/api/v1/cache/entries/{key}",
            get(crate::cachebrowse::detail).delete(crate::cachebrowse::delete_entry),
        )
        .route(
            "/api/v1/cache/entries/{key}/runs",
            get(crate::cachebrowse::entry_runs),
        )
        .route(
            "/api/v1/cache/{key}",
            get(cache_get).head(cache_head).put(cache_put),
        )
        .route("/assets/{*path}", get(serve_asset))
        // The run-set browser and access management, from `crate::sets`.
        .merge(crate::sets::routes())
        .fallback(spa_fallback);

    // The MCP endpoint, only when enabled. Merged before the layers below so
    // it inherits the request id, tracing, decompression, and the auth
    // middleware that resolves the `Identity` its dispatcher authorizes on.
    let api = match state.mcp.as_deref() {
        Some(mcp) => api.merge(crate::mcp::routes(mcp)),
        None => api,
    };

    // Request-id + tracing. axum applies the LAST `.layer()` outermost, so the
    // three request-id/trace layers are added inner-first (propagate, trace,
    // set) to yield the canonical tower-http order a request/response flows
    // through: SetRequestId (assigns a ULID, or keeps an inbound id) is
    // outermost so the id exists before TraceLayer builds its span; TraceLayer
    // reads that id into the span; PropagateRequestId copies the id onto the
    // response before it reaches TraceLayer's `on_response`. Auth, body-limit,
    // and decompression stay inner so their early responses still carry the id.
    let trace = TraceLayer::new_for_http()
        .make_span_with(|req: &Request<Body>| {
            let request_id = req
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-");
            info_span!(
                "http",
                method = %req.method(),
                path = %req.uri().path(),
                request_id = %request_id,
            )
        })
        .on_response(
            |res: &Response<Body>, latency: std::time::Duration, _span: &Span| {
                tracing::info!(
                    status = res.status().as_u16(),
                    latency_ms = latency.as_millis() as u64,
                    "response"
                );
            },
        )
        .on_failure(DefaultOnFailure::new().level(Level::WARN));

    // The CSRF layer is added first (= innermost) so it runs after the auth
    // middleware has attached the Identity it inspects.
    api.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::csrf_middleware,
    ))
    .layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::auth_middleware,
    ))
    .layer(tower_http::decompression::RequestDecompressionLayer::new())
    .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
    .layer(PropagateRequestIdLayer::x_request_id())
    .layer(trace)
    .layer(SetRequestIdLayer::x_request_id(MakeUlidRequestId))
    .with_state(state)
}

/// ULID request-id maker. Only invoked by [`SetRequestIdLayer`] when the request
/// has no `x-request-id` header, so an inbound id is always respected.
#[derive(Clone, Default)]
struct MakeUlidRequestId;

impl MakeRequestId for MakeUlidRequestId {
    fn make_request_id<B>(&mut self, _: &Request<B>) -> Option<RequestId> {
        HeaderValue::from_str(&ulid::Ulid::generate().to_string())
            .ok()
            .map(RequestId::new)
    }
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
    Ok(Json(meta_view(&state).await?).into_response())
}

/// The browsable cache tiers, in the order a switcher should show them.
fn cache_tiers(state: &AppState) -> Vec<CacheTierMeta> {
    let mut tiers = vec![CacheTierMeta {
        id: crate::domain::CacheTier::Server,
        label: "Server".to_string(),
        search: "fts".to_string(),
    }];
    if state.local_cache.is_some() {
        tiers.push(CacheTierMeta {
            id: crate::domain::CacheTier::Local,
            label: "Local disk".to_string(),
            search: "substring".to_string(),
        });
    }
    tiers
}

/// Build the instance-metadata view.
///
/// Factored out of the handler so the MCP `get_server_info` tool answers from
/// the same source rather than assembling a second, drifting copy.
pub(crate) async fn meta_view(state: &AppState) -> anyhow::Result<MetaResponse> {
    let current = RESULT_SCHEMA_VERSION;
    let min = current.saturating_sub(1);
    let supported: Vec<u32> = (min..=current).collect();
    let setup_required = state.storage.count_users().await? == 0;
    Ok(MetaResponse {
        name: "domarinn".to_string(),
        version: domarinn_core::VERSION.to_string(),
        auth_mode: state.auth_mode,
        setup_required,
        sso_providers: state.sso.descriptors(),
        supported_schema_versions: supported,
        result_schema_version: current,
        cache: MetaCacheLimits {
            max_entry_bytes: state.cache_limits.max_entry_bytes,
            max_bytes: state.cache_limits.max_bytes,
            max_age_days: state.cache_limits.max_age_days,
        },
        cache_tiers: cache_tiers(state),
        mcp_enabled: state.mcp.is_some(),
    })
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
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>domarinn</title></head>\
     <body style=\"font-family:system-ui;max-width:40rem;margin:4rem auto;padding:0 1rem\">\
     <h1>domarinn</h1><p>The results UI is not built into this binary yet. \
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
            format!(
                "unsupported schema_version {schema_version}; this server accepts {min}..={current}. \
                 Below {min}: the uploading CLI is older than this server — upgrade the CLI. \
                 Above {current}: this server is older than the CLI — upgrade the server \
                 (or downgrade the CLI to match)."
            ),
        ));
    }

    let run: RunResult = serde_json::from_value(value).map_err(|e| {
        ApiError::status(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("invalid run document: {e}"),
        )
    })?;
    // The global `write` scope got us here; a restricted target needs an
    // `upload` grant on top of it. Checked after parsing, because the target
    // set is a property of the document rather than the URL.
    require_set_access(
        &state,
        &scope.identity,
        run.project.as_deref(),
        run.suite.as_deref(),
        GrantLevel::Upload,
    )
    .await?;

    let run_id = run.run_id.clone();
    let uploaded_by = scope.identity.label.clone();

    let outcome = state.storage.ingest_run(run, uploaded_by).await?;
    let url = build_run_url(&state, &headers, &run_id);

    match outcome {
        IngestOutcome::Created => Ok((
            StatusCode::CREATED,
            Json(IngestResponse { id: run_id, url }),
        )
            .into_response()),
        IngestOutcome::Existing => {
            Ok((StatusCode::OK, Json(IngestResponse { id: run_id, url })).into_response())
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

fn build_run_url(state: &AppState, headers: &HeaderMap, run_id: &RunId) -> String {
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
#[serde(deny_unknown_fields)]
struct RunQuery {
    project: Option<String>,
    suite: Option<String>,
    tag: Option<String>,
    branch: Option<String>,
    since: Option<String>,
    until: Option<String>,
    status: Option<RunStatusFilter>,
    cached: Option<CachedFilter>,
    /// `ci` | `local` — the facet that separates the canonical CI stream from
    /// developer iteration.
    origin: Option<OriginFilter>,
    /// Matches either the recorded actor or the authenticated uploader.
    actor: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_runs(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<RunQuery>,
) -> ApiResult<Response> {
    let filter = RunListFilter {
        visibility: RunVisibility::of(&scope.identity),
        project: q.project,
        suite: q.suite,
        tag: q.tag,
        branch: q.branch,
        since_ms: q.since.as_deref().and_then(storage::parse_time_ms),
        until_ms: q.until.as_deref().and_then(storage::parse_time_ms),
        status: q.status,
        cached: q.cached,
        origin: q.origin,
        actor: q.actor,
        limit: clamp_limit(q.limit),
        cursor: q.cursor.as_deref().and_then(storage::decode_cursor),
    };
    let page = state.storage.list_runs(filter).await?;
    Ok(Json(RunListResponse {
        runs: page.runs,
        next_cursor: page.next_cursor,
        cached_hidden: page.cached_hidden,
    })
    .into_response())
}

async fn get_run(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> ApiResult<Response> {
    // An invisible run is `None` here, so it 404s through exactly the same path
    // as one that never existed — no 403, no existence leak.
    match state
        .storage
        .get_run(id, RunVisibility::of(&scope.identity))
        .await?
    {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Err(not_found("run")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseQuery {
    status: Option<CaseStatus>,
    tag: Option<String>,
    q: Option<String>,
    provider: Option<String>,
    prompt: Option<String>,
    test: Option<String>,
    stop_reason: Option<String>,
    /// Exact-match on the structured failure class.
    error_class: Option<String>,
    /// Exact-match on why the case came back empty.
    empty_reason: Option<String>,
    cached: Option<bool>,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_cases(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(id): Path<RunId>,
    ApiQuery(q): ApiQuery<CaseQuery>,
) -> ApiResult<Response> {
    let visibility = RunVisibility::of(&scope.identity);
    if !state
        .storage
        .run_exists(id.clone(), visibility.clone())
        .await?
    {
        return Err(not_found("run"));
    }
    let filter = CaseListFilter {
        run_id: id,
        visibility,
        status: q.status,
        tag: q.tag,
        q: q.q,
        provider: q.provider,
        prompt: q.prompt,
        test: q.test,
        stop_reason: q.stop_reason,
        error_class: q.error_class,
        empty_reason: q.empty_reason,
        cached: q.cached,
        limit: clamp_limit(q.limit),
        cursor: q.cursor.as_deref().and_then(|c| c.parse::<i64>().ok()),
    };
    let page = state.storage.list_cases(filter).await?;
    Ok(Json(page).into_response())
}

async fn get_case(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((id, case_key)): Path<(RunId, CaseKey)>,
) -> ApiResult<Response> {
    let visibility = RunVisibility::of(&scope.identity);
    match state.storage.get_case(id, case_key, visibility).await? {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Err(not_found("case")),
    }
}

async fn export_run(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> ApiResult<Response> {
    match state
        .storage
        .export_run(id, RunVisibility::of(&scope.identity))
        .await?
    {
        Some(doc) => Ok(Json(doc).into_response()),
        None => Err(not_found("run")),
    }
}

/// Query for `GET /runs/{id}/matrix`. `limit` bounds the test ROWS per page;
/// columns are always complete.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatrixQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn run_matrix(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(id): Path<RunId>,
    ApiQuery(q): ApiQuery<MatrixQuery>,
) -> ApiResult<Response> {
    // An unknown (or invisible) run is a 404; a known run whose cases all lack
    // cell columns returns an empty matrix (handled by the aggregation).
    let visibility = RunVisibility::of(&scope.identity);
    if !state
        .storage
        .run_exists(id.clone(), visibility.clone())
        .await?
    {
        return Err(not_found("run"));
    }
    let filter = MatrixFilter {
        run_id: id,
        visibility,
        limit: q
            .limit
            .unwrap_or(MATRIX_DEFAULT_LIMIT)
            .clamp(1, MATRIX_MAX_LIMIT),
        cursor: q.cursor.as_deref().and_then(|c| c.parse::<i64>().ok()),
    };
    let matrix = state.storage.run_matrix(filter).await?;
    Ok(Json(matrix).into_response())
}

async fn get_run_config(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> ApiResult<Response> {
    match state
        .storage
        .get_run_config(id, RunVisibility::of(&scope.identity))
        .await?
    {
        Some(config) => Ok(Json(config).into_response()),
        None => Err(not_found("run")),
    }
}

async fn compare_runs(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((id, other)): Path<(RunId, RunId)>,
) -> ApiResult<Response> {
    match state
        .storage
        .compare_runs(id, other, RunVisibility::of(&scope.identity))
        .await?
    {
        Some(cmp) => Ok(Json(cmp).into_response()),
        None => Err(not_found("run")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery {
    q: String,
    limit: Option<i64>,
    /// Narrow hits by the owning run's cache provenance. Absent is a no-op —
    /// the hiding default lives in the web client, not here, so an API caller
    /// that says nothing keeps getting everything.
    cached: Option<CachedFilter>,
}

async fn search(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<SearchQuery>,
) -> ApiResult<Response> {
    let limit = clamp_limit(q.limit);
    let res: SearchResponse = state
        .storage
        .search(q.q, limit, RunVisibility::of(&scope.identity), q.cached)
        .await?;
    Ok(Json(res).into_response())
}

async fn delete_run(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path(id): Path<RunId>,
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

async fn list_projects(scope: Scoped<Read>, State(state): State<AppState>) -> ApiResult<Response> {
    let projects = state
        .storage
        .list_projects(RunVisibility::of(&scope.identity))
        .await?;
    Ok(Json(projects).into_response())
}

async fn list_suites(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> ApiResult<Response> {
    let suites = state
        .storage
        .list_suites(project, RunVisibility::of(&scope.identity))
        .await?;
    Ok(Json(suites).into_response())
}

#[derive(Debug, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaselineBody {
    run_id: RunId,
}

async fn put_baseline(
    scope: Scoped<Write>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
    ApiJson(body): ApiJson<BaselineBody>,
) -> ApiResult<Response> {
    require_set_access(
        &state,
        &scope.identity,
        Some(&project),
        Some(&suite),
        GrantLevel::Upload,
    )
    .await?;
    if state
        .storage
        .set_baseline(
            project.clone(),
            suite.clone(),
            body.run_id.clone(),
            RunVisibility::of(&scope.identity),
        )
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
    scope: Scoped<Write>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
) -> ApiResult<Response> {
    require_set_access(
        &state,
        &scope.identity,
        Some(&project),
        Some(&suite),
        GrantLevel::Upload,
    )
    .await?;
    if state.storage.delete_baseline(project, suite).await? {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(not_found("baseline"))
    }
}

/// Query for `GET /projects/{project}/suites/{suite}/cases/{case_key}/history`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    limit: Option<i64>,
}

async fn case_history(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, suite, case_key)): Path<(String, String, CaseKey)>,
    ApiQuery(q): ApiQuery<HistoryQuery>,
) -> ApiResult<Response> {
    let limit = q
        .limit
        .unwrap_or(HISTORY_DEFAULT_LIMIT)
        .clamp(1, HISTORY_MAX_LIMIT);
    match state
        .storage
        .case_history(
            project,
            suite,
            case_key,
            limit,
            RunVisibility::of(&scope.identity),
        )
        .await?
    {
        Some(history) => Ok(Json(history).into_response()),
        None => Err(not_found("case")),
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

pub(crate) fn validate_cache_key(key: &str) -> ApiResult<()> {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn clamp_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Gate a write on the target run set at `needed`, on top of whatever global
/// scope the route already demanded.
///
/// A 403 rather than a 404: unlike a run id, the `(project, suite)` pair here
/// comes from the caller, so refusing it discloses only that the set they named
/// is restricted — an accepted bit — and a silent 404 on a legitimate upload
/// would be a debugging trap for whoever runs the CI job. The access endpoints
/// in [`crate::sets`] make the opposite trade, and say why.
async fn require_set_access(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: Option<&str>,
    suite: Option<&str>,
    needed: GrantLevel,
) -> ApiResult<()> {
    let allowed = state
        .storage
        .set_access(
            RunVisibility::of(identity),
            project.map(str::to_string),
            suite.map(str::to_string),
            needed,
        )
        .await?;
    if allowed {
        return Ok(());
    }
    // The article agrees with the level: this body is on the wire, in front of
    // every CI log that hits a set it cannot write.
    let article = match needed {
        GrantLevel::Upload => "an",
        GrantLevel::View | GrantLevel::Manage => "a",
    };
    Err(ApiError::status(
        StatusCode::FORBIDDEN,
        format!("this run set is restricted; you need {article} {needed} grant on it"),
    ))
}

pub(crate) fn not_found(what: &str) -> ApiError {
    ApiError::status(StatusCode::NOT_FOUND, format!("{what} not found"))
}
