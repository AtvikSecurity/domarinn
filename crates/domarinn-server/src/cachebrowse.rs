//! Handlers for browsing the cache.
//!
//! Separate from [`crate::routes`], which is near the per-file line ratchet,
//! and wired from its router the way [`crate::accounts`] already is.
//!
//! # Why listing is admin-scoped and reading one entry is not
//!
//! `GET /cache/{key}` has always been `read`, but a key is
//! `sha256(canonical_json({request, repeat, salts}))` — the only way to compute
//! one is to already possess the exact prompt, model and parameters. Read scope
//! alone therefore gets you nothing you did not already have; the cache is
//! capability-protected by accident, and [`detail`] keeps that exactly as it is.
//!
//! *Enumerating* is the new capability, and it is a different thing: it turns
//! "you already know the prompt" into every prompt, every response, sorted by
//! cost and searchable. Two facts settle where that belongs. `read` is the
//! **anonymous** scope — `protect-writes` grants it without a token — so
//! listing at `read` would publish the whole corpus to unauthenticated callers.
//! And `write` is the CI-token scope, the most-copied credential in any
//! deployment. So [`list`] and [`facets`] are `admin`, which `POST
//! /cache/prune` already requires: destroying the corpus and enumerating it are
//! comparable powers. Widening a gate later is non-breaking; narrowing one is
//! not.

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::auth::{Admin, Read, Scoped};
use crate::domain::{CacheSort, CacheTier, SortOrder};
use crate::extract::ApiQuery;
use crate::routes::{clamp_limit, not_found, validate_cache_key, ApiResult};
use crate::storage::{decode_entry_cursor, parse_time_ms, CacheListFilter};
use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListQuery {
    tier: Option<CacheTier>,
    /// A recorded kind, or the `unindexed` / `unparseable` pseudo-values.
    kind: Option<String>,
    model: Option<String>,
    /// Free text over the request and output. Quoted term-by-term before it
    /// reaches fts5, so no input can be a syntax error.
    q: Option<String>,
    since: Option<String>,
    until: Option<String>,
    min_cost_microusd: Option<i64>,
    max_cost_microusd: Option<i64>,
    sort: Option<CacheSort>,
    order: Option<SortOrder>,
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DetailQuery {
    tier: Option<CacheTier>,
    /// Include the provider's raw metadata. Off by default: it is the largest
    /// member of an entry and the least often wanted.
    raw: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FacetQuery {
    tier: Option<CacheTier>,
}

/// Resolve the requested tier, or explain why it is not here.
///
/// The single place tier dispatch lives, so adding one is a change here rather
/// than in three handlers. A tier that is not mounted is a `404`, not a `400`:
/// the request is well-formed and the tier is a real one, it simply is not
/// present on this server. `/meta` advertises which tiers are.
fn resolve_tier(state: &AppState, tier: Option<CacheTier>) -> ApiResult<CacheTier> {
    match tier.unwrap_or_default() {
        CacheTier::Server => Ok(CacheTier::Server),
        CacheTier::Local if state.local_cache.is_some() => Ok(CacheTier::Local),
        CacheTier::Local => Err(not_found("local cache tier")),
    }
}

pub(crate) async fn list(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Response> {
    let tier = resolve_tier(&state, q.tier)?;
    let filter = CacheListFilter {
        kind: q.kind,
        model: q.model,
        q: q.q,
        since: q.since.as_deref().and_then(parse_time_ms),
        until: q.until.as_deref().and_then(parse_time_ms),
        min_cost_microusd: q.min_cost_microusd,
        max_cost_microusd: q.max_cost_microusd,
        sort: q.sort.unwrap_or_default(),
        order: q.order.unwrap_or_default(),
        limit: clamp_limit(q.limit),
        cursor: q.cursor.as_deref().and_then(decode_entry_cursor),
    };
    let page = match (tier, &state.local_cache) {
        (CacheTier::Local, Some(local)) => local.list(filter).await?,
        _ => state.storage.cache_list_entries(filter).await?,
    };
    Ok(Json(page).into_response())
}

pub(crate) async fn detail(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(key): Path<String>,
    ApiQuery(q): ApiQuery<DetailQuery>,
) -> ApiResult<Response> {
    validate_cache_key(&key)?;
    let tier = resolve_tier(&state, q.tier)?;
    let include_raw = q.raw.unwrap_or(false);
    let found = match (tier, &state.local_cache) {
        (CacheTier::Local, Some(local)) => local.detail(&key, include_raw).await?,
        _ => state.storage.cache_entry_detail(key, include_raw).await?,
    };
    match found {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Err(not_found("cache entry")),
    }
}

pub(crate) async fn facets(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<FacetQuery>,
) -> ApiResult<Response> {
    let tier = resolve_tier(&state, q.tier)?;
    let facets = match (tier, &state.local_cache) {
        (CacheTier::Local, Some(local)) => local.facets().await?,
        _ => state.storage.cache_facets().await?,
    };
    Ok(Json(facets).into_response())
}
