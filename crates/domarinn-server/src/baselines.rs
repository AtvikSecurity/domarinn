//! Baseline pin management and resolution: `/projects/{p}/suites/{s}/baseline`.
//!
//! A suite's baseline is pinned to a fixed run *or* a branch
//! ([`BaselinePin`]). `GET .../baseline/export` is the one endpoint the CLI's
//! `--against server:baseline` and `--against server:branch:<name>` resolve
//! through: it returns a full run document — the pinned run's stored blob, or
//! a composite merged from the branch's newest runs.
//!
//! Every "there is nothing to compare against" 404 here carries a stable
//! machine `code` ([`ApiError::Coded`]). The CLI keys its absent-vs-failed
//! split off that code: a recognized code is an absence (first run, nothing
//! pinned yet — carry on), while a *bare* 404 means the route itself is
//! missing (an older server) and must not silently skip the gate.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use ts_rs::TS;

use domarinn_core::ids::RunId;

use crate::auth::{Read, Scoped, Write};
use crate::extract::{ApiJson, ApiQuery};
use crate::routes::{not_found, require_set_access, ApiError, ApiResult};
use crate::runsets::{GrantLevel, RunVisibility};
use crate::storage::BaselinePin;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project}/suites/{suite}/baseline",
            get(get_baseline).put(put_baseline).delete(delete_baseline),
        )
        .route(
            "/api/v1/projects/{project}/suites/{suite}/baseline/export",
            get(export_baseline),
        )
}

/// `PUT .../baseline` body: exactly one of the two pin kinds.
#[derive(Debug, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaselineBody {
    #[ts(optional)]
    run_id: Option<RunId>,
    #[ts(optional)]
    branch: Option<String>,
}

/// Query for `GET .../baseline/export`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportQuery {
    /// Resolve this branch directly, pin or no pin (`server:branch:<name>`).
    branch: Option<String>,
    /// The run being gated — dropped from a composite so a head run that
    /// uploaded before comparing cannot become its own baseline.
    exclude: Option<RunId>,
}

/// An absence the caller may reasonably continue past, spelled with the
/// machine `code` the CLI keys that decision off.
fn absent(code: &'static str, message: String) -> ApiError {
    ApiError::Coded(StatusCode::NOT_FOUND, code, message)
}

async fn get_baseline(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
) -> ApiResult<Response> {
    let meta = state
        .storage
        .baseline_meta(
            project.clone(),
            suite.clone(),
            RunVisibility::of(&scope.identity),
        )
        .await?;
    match meta {
        Some((pin, set_at)) => {
            let (run_id, branch) = match pin {
                BaselinePin::Run(id) => (Some(id), None),
                BaselinePin::Branch(b) => (None, Some(b)),
            };
            Ok(Json(json!({
                "project": project,
                "suite": suite,
                "run_id": run_id,
                "branch": branch,
                "set_at": set_at,
            }))
            .into_response())
        }
        None => Err(absent(
            "baseline_unpinned",
            format!("no baseline pinned for {project}/{suite}"),
        )),
    }
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
    let pin = match (body.run_id, body.branch) {
        (Some(run_id), None) => BaselinePin::Run(run_id),
        (None, Some(branch)) => {
            let branch = branch.trim();
            if branch.is_empty() {
                return Err(ApiError::status(
                    StatusCode::BAD_REQUEST,
                    "branch must be a non-empty name",
                ));
            }
            BaselinePin::Branch(branch.to_string())
        }
        _ => {
            return Err(ApiError::status(
                StatusCode::BAD_REQUEST,
                "exactly one of `run_id` or `branch` must be set",
            ));
        }
    };
    let echo = match &pin {
        BaselinePin::Run(id) => json!({ "project": project, "suite": suite, "run_id": id }),
        BaselinePin::Branch(b) => json!({ "project": project, "suite": suite, "branch": b }),
    };
    if state
        .storage
        .set_baseline(
            project.clone(),
            suite.clone(),
            pin,
            RunVisibility::of(&scope.identity),
        )
        .await?
    {
        Ok(Json(echo).into_response())
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

async fn export_baseline(
    scope: Scoped<Read>,
    State(state): State<AppState>,
    Path((project, suite)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<ExportQuery>,
) -> ApiResult<Response> {
    let vis = RunVisibility::of(&scope.identity);
    // An explicit `?branch=` overrides any pin: the caller named the reference.
    if let Some(branch) = query.branch {
        return branch_composite(&state, project, suite, branch, query.exclude, vis).await;
    }
    match state
        .storage
        .baseline_pin(project.clone(), suite.clone(), vis.clone())
        .await?
    {
        Some(BaselinePin::Run(id)) => {
            match state.storage.export_run(id, vis).await? {
                Some(doc) => Ok(Json(doc).into_response()),
                // `baseline_pin` already hides an invisible pinned run, so this
                // is only a delete race — still an absence, not a server fault.
                None => Err(absent(
                    "baseline_unpinned",
                    format!("the baseline pinned for {project}/{suite} no longer exists"),
                )),
            }
        }
        Some(BaselinePin::Branch(branch)) => {
            branch_composite(&state, project, suite, branch, query.exclude, vis).await
        }
        None => Err(absent(
            "baseline_unpinned",
            format!("no baseline pinned for {project}/{suite}"),
        )),
    }
}

async fn branch_composite(
    state: &AppState,
    project: String,
    suite: String,
    branch: String,
    exclude: Option<RunId>,
    vis: RunVisibility,
) -> ApiResult<Response> {
    match state
        .storage
        .branch_baseline_export(project.clone(), suite.clone(), branch.clone(), exclude, vis)
        .await?
    {
        Some(doc) => Ok(Json(doc).into_response()),
        None => Err(absent(
            "no_runs_on_branch",
            format!("no runs of {project}/{suite} on branch {branch} to merge into a baseline"),
        )),
    }
}
