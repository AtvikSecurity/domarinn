//! Handlers for the run-set browser and its access lists (`/api/v1/sets*`).
//!
//! Separate from [`crate::routes`], which is at the per-file line ratchet, and
//! wired from its router the way [`crate::cachebrowse`] already is. Everything
//! here is additive: `/api/v1/projects*` keeps the wire shape older CLIs
//! depend on, and this surface is free to grow.
//!
//! # Three gates, three status codes
//!
//! * **Reads of the browser** ([`list`], [`project`], [`suite`]) are
//!   `Scoped<Read>` and filtered by [`RunVisibility`]. An invisible set is
//!   `None` from storage and so 404s through exactly the same path as one that
//!   never existed — no 403, no existence leak. Identical to the run reads.
//!
//! * **Reads and writes of a set's access list** need
//!   [`GrantLevel::Manage`] over it, and a refusal is a **404**. That is not
//!   politeness: the access list names the users who can reach a restricted
//!   set, so "you may not see this" and "there is nothing here" must be the
//!   same answer. The manage level is never granted by the default-open waiver
//!   (see [`crate::storage::Storage::set_access`]), so an unrestricted set is
//!   just as closed here as a locked one — a caller who could otherwise upload
//!   into it still cannot read or edit who else may.
//!
//! * **Restriction toggling** is `Scoped<Admin>`, so a manage-grant holder gets
//!   a **403**: they administer their set's access list, but locking a set (and
//!   unlocking one) stays with the operator. The 403 is safe precisely because
//!   the caller already proved, by holding the manage grant, that they know the
//!   set exists.
//!
//! # Why the grant mutations are `Scoped<Read>`
//!
//! They are writes, but the route scope is not their gate — the covering manage
//! grant is, and it is strictly stronger: no auth mode grants it, and
//! [`RunVisibility::Public`] can never satisfy it, so an anonymous or
//! static-token caller is refused in every mode. Layering `Scoped<Write>` on
//! top would subtract capability rather than add safety: it would stop a
//! viewer-role account that was deliberately given `manage` over one set from
//! using it. CSRF is unaffected — that middleware keys on the HTTP method.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};

use crate::auth::{Admin, Identity, Read, Scoped};
use crate::domain::UserId;
use crate::dto::sets::{SetAccessResponse, SetGrantUpsert, SetGrantView};
use crate::extract::ApiJson;
use crate::routes::{not_found, ApiResult};
use crate::runsets::{GrantLevel, RunVisibility};
use crate::AppState;

/// The run-set routes, merged into the main router before its layers are
/// applied so they inherit auth, CSRF, tracing, and the request id.
///
/// The literal `access` / `restriction` / `grants` segments sit where a suite
/// name could otherwise go, which is why the suite forms spell `/suites/`
/// explicitly: `/sets/{project}/suites/{suite}` can never collide with
/// `/sets/{project}/access`, whatever a project or suite is called.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sets", get(list))
        .route("/api/v1/sets/{project}", get(project))
        .route("/api/v1/sets/{project}/access", get(project_access))
        .route(
            "/api/v1/sets/{project}/restriction",
            put(project_restrict).delete(project_unrestrict),
        )
        .route(
            "/api/v1/sets/{project}/grants/{user_id}",
            put(project_grant).delete(project_ungrant),
        )
        .route("/api/v1/sets/{project}/suites/{suite}", get(suite))
        .route(
            "/api/v1/sets/{project}/suites/{suite}/access",
            get(suite_access),
        )
        .route(
            "/api/v1/sets/{project}/suites/{suite}/restriction",
            put(suite_restrict).delete(suite_unrestrict),
        )
        .route(
            "/api/v1/sets/{project}/suites/{suite}/grants/{user_id}",
            put(suite_grant).delete(suite_ungrant),
        )
}

// ---------------------------------------------------------------------------
// Browse
// ---------------------------------------------------------------------------

async fn list(scope: Scoped<Read>, State(state): State<AppState>) -> ApiResult<Response> {
    let sets = state
        .storage
        .list_run_sets(RunVisibility::of(&scope.identity))
        .await?;
    Ok(Json(sets).into_response())
}

async fn project(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> ApiResult<Response> {
    match state
        .storage
        .run_set_project(project, RunVisibility::of(&scope.identity))
        .await?
    {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Err(not_found("run set")),
    }
}

async fn suite(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
) -> ApiResult<Response> {
    match state
        .storage
        .run_set_suite(project, suite, RunVisibility::of(&scope.identity))
        .await?
    {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Err(not_found("run set")),
    }
}

// ---------------------------------------------------------------------------
// Access lists
// ---------------------------------------------------------------------------

/// Refuse unless this caller may administer `(project, suite)`'s access list.
///
/// The refusal is [`not_found`] — see this module's header for why a 403 would
/// be a disclosure.
async fn require_manage(
    state: &AppState,
    identity: &Identity,
    project: &str,
    suite: Option<&str>,
) -> ApiResult<()> {
    let allowed = state
        .storage
        .set_access(
            RunVisibility::of(identity),
            Some(project.to_string()),
            suite.map(str::to_string),
            GrantLevel::Manage,
        )
        .await?;
    if allowed {
        Ok(())
    } else {
        Err(not_found("run set"))
    }
}

async fn access(
    state: AppState,
    identity: Identity,
    project: String,
    suite: Option<String>,
) -> ApiResult<Response> {
    require_manage(&state, &identity, &project, suite.as_deref()).await?;
    // Exact scope on both halves: this is the editor for the row the toggle
    // beside it writes, not a report on what covers the set.
    let restricted = state
        .storage
        .run_set_restricted(Some(project.clone()), suite.clone())
        .await?;
    let grants = state
        .storage
        .list_run_set_grants(project, suite)
        .await?
        .into_iter()
        .map(|g| SetGrantView {
            user_id: g.user_id,
            username: g.username,
            level: g.level,
            created_at: g.created_at,
            created_by: g.created_by,
        })
        .collect();
    Ok(Json(SetAccessResponse { restricted, grants }).into_response())
}

async fn project_access(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> ApiResult<Response> {
    access(state, scope.identity, project, None).await
}

async fn suite_access(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
) -> ApiResult<Response> {
    access(state, scope.identity, project, Some(suite)).await
}

// ---------------------------------------------------------------------------
// Restriction
// ---------------------------------------------------------------------------

/// A restriction row is a fact about a `(project, suite)` pair, not about the
/// runs in it: it may be created before the set has ever been uploaded to, so
/// nothing here checks that the set exists in `runs`.
async fn restrict(
    state: AppState,
    identity: Identity,
    project: String,
    suite: Option<String>,
) -> ApiResult<Response> {
    // Idempotent: `false` means the row was already there, which is the same
    // outcome the caller asked for.
    state
        .storage
        .restrict_run_set(project, suite, identity.label.clone())
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn unrestrict(
    state: AppState,
    project: String,
    suite: Option<String>,
) -> ApiResult<Response> {
    if state.storage.unrestrict_run_set(project, suite).await? {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(not_found("restriction"))
    }
}

async fn project_restrict(
    scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> ApiResult<Response> {
    restrict(state, scope.identity, project, None).await
}

async fn suite_restrict(
    scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
) -> ApiResult<Response> {
    restrict(state, scope.identity, project, Some(suite)).await
}

async fn project_unrestrict(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> ApiResult<Response> {
    unrestrict(state, project, None).await
}

async fn suite_unrestrict(
    _scope: Scoped<Admin>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
) -> ApiResult<Response> {
    unrestrict(state, project, Some(suite)).await
}

// ---------------------------------------------------------------------------
// Grants
// ---------------------------------------------------------------------------

async fn grant(
    state: AppState,
    identity: Identity,
    project: String,
    suite: Option<String>,
    user_id: UserId,
    level: GrantLevel,
) -> ApiResult<Response> {
    require_manage(&state, &identity, &project, suite.as_deref()).await?;
    // A grant to a user that does not exist would be unreachable dead policy —
    // and `run_set_grants` has no foreign key onto `users`, so nothing else
    // would catch it. The 404 names the user, not the set, so it cannot be
    // confused with the manage refusal above.
    if state
        .storage
        .get_user_by_id(user_id.clone())
        .await?
        .is_none()
    {
        return Err(not_found("user"));
    }
    state
        .storage
        .upsert_run_set_grant(project, suite, user_id, level, identity.label.clone())
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn ungrant(
    state: AppState,
    identity: Identity,
    project: String,
    suite: Option<String>,
    user_id: UserId,
) -> ApiResult<Response> {
    require_manage(&state, &identity, &project, suite.as_deref()).await?;
    if state
        .storage
        .delete_run_set_grant(project, suite, user_id)
        .await?
    {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(not_found("grant"))
    }
}

async fn project_grant(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, user_id)): Path<(String, UserId)>,
    ApiJson(body): ApiJson<SetGrantUpsert>,
) -> ApiResult<Response> {
    grant(state, scope.identity, project, None, user_id, body.level).await
}

async fn suite_grant(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, suite, user_id)): Path<(String, String, UserId)>,
    ApiJson(body): ApiJson<SetGrantUpsert>,
) -> ApiResult<Response> {
    grant(
        state,
        scope.identity,
        project,
        Some(suite),
        user_id,
        body.level,
    )
    .await
}

async fn project_ungrant(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, user_id)): Path<(String, UserId)>,
) -> ApiResult<Response> {
    ungrant(state, scope.identity, project, None, user_id).await
}

async fn suite_ungrant(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, suite, user_id)): Path<(String, String, UserId)>,
) -> ApiResult<Response> {
    ungrant(state, scope.identity, project, Some(suite), user_id).await
}
