//! Handlers for browsing and administering the cache.
//!
//! Separate from [`crate::routes`], which is near the per-file line ratchet,
//! and wired from its router the way [`crate::accounts`] already is. The
//! stats/prune/delete surface lives here beside the browse surface rather than
//! back in `routes`, because finding an entry and destroying it are one
//! workflow — you prune what a filter just showed you.
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
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::auth::{Admin, Identity, Read, Scoped};
use crate::domain::{CacheSort, CacheTier, SortOrder};
use crate::dto::cache::PruneResponse;
use crate::extract::ApiQuery;
use crate::routes::{clamp_limit, not_found, validate_cache_key, ApiError, ApiResult};
use crate::runsets::RunVisibility;
use crate::storage::{
    decode_entry_cursor, decode_run_cursor, parse_time_ms, CacheListFilter, CachePruneFilter,
};
use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListQuery {
    tier: Option<CacheTier>,
    /// A recorded kind, or the `unindexed` / `unparseable` pseudo-values.
    kind: Option<String>,
    model: Option<String>,
    /// One recorded `empty_reason`. Never folded into `q`: it is a facet, and a
    /// free-text match would also hit every entry whose *output* discusses one.
    empty_reason: Option<String>,
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
pub(crate) struct EntryRunsQuery {
    limit: Option<i64>,
    cursor: Option<String>,
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
        empty_reason: q.empty_reason,
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

/// Which runs used this entry.
///
/// `Scoped<Read>`, matching [`detail`] rather than [`list`]: it is addressed by
/// a key you must already know, and the runs it names are already browsable at
/// the same scope. Which is why the page is also filtered by run visibility —
/// "already browsable" has to keep meaning what it says once run sets exist.
pub(crate) async fn entry_runs(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(key): Path<String>,
    ApiQuery(q): ApiQuery<EntryRunsQuery>,
) -> ApiResult<Response> {
    validate_cache_key(&key)?;
    let page = state
        .storage
        .cache_entry_runs(
            key,
            clamp_limit(q.limit),
            q.cursor.as_deref().and_then(decode_run_cursor),
            RunVisibility::of(&scope.identity),
        )
        .await?;
    Ok(Json(page).into_response())
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

// ---------------------------------------------------------------------------
// Administering: stats, delete, prune
// ---------------------------------------------------------------------------

pub(crate) async fn cache_stats(
    _scope: Scoped<Read>,
    State(state): State<AppState>,
) -> ApiResult<Response> {
    Ok(Json(state.storage.cache_stats().await?).into_response())
}

/// Who is doing this, for the log line.
///
/// Label first (a username, or a static token's scope name), falling back to
/// the authenticator that resolved the request — which under `AuthMode::Open`
/// is `anonymous`, and saying so is the point.
fn actor(identity: &Identity) -> String {
    match identity.label.as_deref() {
        Some(label) => format!("{label} ({})", identity.source.as_str()),
        None => identity.source.as_str().to_string(),
    }
}

/// Delete one entry.
///
/// `Scoped<Admin>`, matching [`cache_prune`] rather than [`detail`]. The
/// asymmetry is deliberate and is [`list`]'s argument run the other way:
/// *reading* an entry you already hold the key for tells you nothing you did
/// not already have, so it stays at `read`; *destroying* rows out of a corpus
/// several people share is the same power a prune has, applied one row at a
/// time and therefore quieter. Widening a gate later is non-breaking.
///
/// `204` when it went, `404` when there was nothing there — so a script can
/// tell "I removed it" from "it was already gone", which a blanket `204` would
/// collapse into one answer.
pub(crate) async fn delete_entry(
    scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Response> {
    // Before the storage call, as `detail` does: a malformed key is the
    // caller's mistake, and answering it with a `404` from a lookup that could
    // never match would read as "that entry does not exist".
    validate_cache_key(&key)?;
    if !state.storage.cache_delete_entry(key.clone()).await? {
        return Err(not_found("cache entry"));
    }
    // The server has no audit log and no rate limiting on this route. This line
    // is the entire forensic record of who removed what, so it is `info!` and
    // it names the actor.
    tracing::info!(actor = %actor(&scope.identity), %key, "cache entry deleted");
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Filters for `POST /cache/prune`.
///
/// `deny_unknown_fields` is load-bearing rather than tidy: without it
/// `?empty_reasons=refusal` — the plural typo — stops being a `400` and becomes
/// a prune that named nothing, which then falls through to the configured
/// retention limits and evicts on age and size instead. A destructive verb must
/// never quietly do something other than what was asked.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PruneQuery {
    older_than_days: Option<i64>,
    newer_than_days: Option<i64>,
    /// Comma-separated, so one query parameter carries a set. The client side
    /// joins on `,`; splitting here is the only place that contract lives.
    empty_reason: Option<String>,
    model: Option<String>,
    kind: Option<String>,
    target_bytes: Option<i64>,
}

/// Evict entries.
///
/// # A prune applies only what it names
///
/// A bare prune — no parameter of any kind, which is what the UI's "Prune
/// cache" button and a plain `POST /cache/prune` send — means "apply the
/// configured retention limits", the manual equivalent of the hourly task.
/// Without that, an unparameterized prune would silently evict nothing.
///
/// But that fallback keys off *nothing at all being named*, not off the two
/// original parameters being absent. Otherwise `?empty_reason=refusal` would
/// **also** apply `max_age_days` and `max_bytes`, and someone reaching for a
/// scalpel would get the blunt instrument they were trying to avoid.
pub(crate) async fn cache_prune(
    scope: Scoped<Admin>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<PruneQuery>,
) -> ApiResult<Response> {
    // A negative window puts the cutoff in the *future*, so `older_than_days=-1`
    // matches every entry ever stored. Live today, and easier to reach by
    // accident now that there are two day parameters to mistype.
    for (name, value) in [
        ("older_than_days", q.older_than_days),
        ("newer_than_days", q.newer_than_days),
    ] {
        if value.is_some_and(|days| days < 0) {
            return Err(ApiError::status(
                StatusCode::BAD_REQUEST,
                format!("{name} must not be negative"),
            ));
        }
    }

    let filter = CachePruneFilter {
        older_than_days: q.older_than_days,
        newer_than_days: q.newer_than_days,
        empty_reason: split_csv(q.empty_reason.as_deref()),
        model: q.model,
        kind: q.kind,
        target_bytes: q.target_bytes,
    };
    let filter = if filter.is_empty() {
        CachePruneFilter {
            older_than_days: Some(state.cache_limits.max_age_days as i64),
            target_bytes: Some(state.cache_limits.max_bytes as i64),
            ..CachePruneFilter::default()
        }
    } else {
        filter
    };

    tracing::info!(
        actor = %actor(&scope.identity),
        filter = ?filter,
        "cache prune requested"
    );
    let pruned = state.storage.cache_prune(filter).await?;
    Ok(Json(PruneResponse { pruned }).into_response())
}

/// Split a comma-separated parameter, dropping empties.
///
/// `?empty_reason=` and `?empty_reason=,,` both yield no values, which
/// [`CachePruneFilter::is_empty`] then reads as "named nothing" — the same
/// answer as omitting the parameter, and the only one that does not turn a
/// stray comma into a wider eviction than was asked for.
fn split_csv(raw: Option<&str>) -> Vec<String> {
    raw.into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comma_separated_reason_list_never_widens_into_no_filter() {
        assert_eq!(split_csv(None), Vec::<String>::new());
        assert_eq!(split_csv(Some("")), Vec::<String>::new());
        assert_eq!(split_csv(Some(" , ,")), Vec::<String>::new());
        assert_eq!(
            split_csv(Some("refusal, content_filter")),
            vec!["refusal".to_string(), "content_filter".to_string()]
        );
    }
}
