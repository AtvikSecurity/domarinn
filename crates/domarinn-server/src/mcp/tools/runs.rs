//! Run-level tools: `find_runs` and `get_run`.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{clamp_limit, parse_args, read_only_annotations, ToolResult};
use crate::domain::{CachedFilter, OriginFilter, RunStatusFilter};
use crate::mcp::budget::Budget;
use crate::mcp::text;
use crate::runsets::RunVisibility;
use crate::storage::{self, MatrixFilter, RunListFilter};
use crate::AppState;
use domarinn_core::ids::RunId;

const FIND_DEFAULT_LIMIT: i64 = 10;
const FIND_MAX_LIMIT: i64 = 50;
/// Matrix rows are quadratic in rows × columns, so this window is much
/// tighter than the REST endpoint's 100/500.
const MATRIX_DEFAULT_LIMIT: i64 = 25;
const MATRIX_MAX_LIMIT: i64 = 100;

/// How `find_runs` groups its answer. Without `group_by` it lists runs; with
/// it, it returns the project/suite catalog instead — which is why
/// `GET /projects` and `GET /projects/{p}/suites` do not need tool slots of
/// their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum GroupBy {
    Project,
    Suite,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FindRunsArgs {
    pub project: Option<String>,
    pub suite: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub actor: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub status: Option<RunStatusFilter>,
    pub cached: Option<CachedFilter>,
    pub origin: Option<OriginFilter>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub group_by: Option<GroupBy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum RunInclude {
    Matrix,
    Config,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GetRunArgs {
    pub run_id: String,
    pub include: Option<Vec<RunInclude>>,
    pub matrix_limit: Option<i64>,
    pub matrix_cursor: Option<i64>,
}

pub(super) fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "find_runs",
            "title": "domarinn: find eval runs",
            "description": "List eval runs newest-first, with the same filters as GET /api/v1/runs. \
                Start here to orient yourself. Set group_by=project or group_by=suite to get the \
                catalog of what exists instead of individual runs (group_by=suite also needs \
                project). Returns compact summaries: pass/fail/error counts, pass rate, cost, and \
                git context. Use get_run for one run's detail.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Exact project name." },
                    "suite": { "type": "string", "description": "Exact suite name." },
                    "tag": { "type": "string", "description": "Runs carrying this tag." },
                    "branch": { "type": "string", "description": "Exact git branch." },
                    "actor": { "type": "string", "description": "Who ran or uploaded it." },
                    "since": { "type": "string", "description": "Lower time bound, RFC3339 or epoch-ms." },
                    "until": { "type": "string", "description": "Upper time bound, RFC3339 or epoch-ms." },
                    "status": {
                        "type": "string", "enum": ["pass", "fail", "error"],
                        "description": "The run's overall verdict."
                    },
                    "cached": {
                        "type": "string", "enum": ["exclude", "only", "all"],
                        "description": "'exclude' hides fully-cached passing runs (replay noise)."
                    },
                    "origin": {
                        "type": "string", "enum": ["ci", "local"],
                        "description": "CI runs versus local developer iteration."
                    },
                    "limit": {
                        "type": "integer", "minimum": 1, "maximum": FIND_MAX_LIMIT,
                        "description": "Max runs to return. Default 10."
                    },
                    "cursor": { "type": "string", "description": "next_cursor from a previous call." },
                    "group_by": {
                        "type": "string", "enum": ["project", "suite"],
                        "description": "Return the catalog of projects or suites instead of runs."
                    }
                },
                "additionalProperties": false
            },
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_run",
            "title": "domarinn: get one run",
            "description": "One run's detail: totals, pass rate, token and cost accounting, git and \
                CI provenance. Pass include=[\"matrix\"] for the prompt x provider pass-rate grid \
                (the fastest way to see which cell is failing) and include=[\"config\"] for the \
                suite config snapshot it ran against. Use list_cases to enumerate individual cases.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "The run's id." },
                    "include": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["matrix", "config"] },
                        "description": "Extra sections to embed. Default: none."
                    },
                    "matrix_limit": {
                        "type": "integer", "minimum": 1, "maximum": MATRIX_MAX_LIMIT,
                        "description": "Max matrix rows when include has 'matrix'. Default 25."
                    },
                    "matrix_cursor": {
                        "type": "integer",
                        "description": "next_cursor from a previous matrix page."
                    }
                },
                "required": ["run_id"],
                "additionalProperties": false
            },
            "annotations": read_only_annotations(),
        }),
    ]
}

pub(super) async fn find_runs(state: &AppState, vis: &RunVisibility, args: Value) -> ToolResult {
    let args: FindRunsArgs = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return e,
    };

    match args.group_by {
        Some(GroupBy::Project) => return list_projects(state, vis).await,
        Some(GroupBy::Suite) => {
            let Some(project) = args.project.clone() else {
                return ToolResult::error(
                    "group_by=suite requires a project argument. Call group_by=project first to \
                     see which projects exist.",
                );
            };
            return list_suites(state, vis, project).await;
        }
        None => {}
    }

    let since_ms = match parse_time(args.since.as_deref(), "since") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let until_ms = match parse_time(args.until.as_deref(), "until") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let cursor = match args.cursor.as_deref().map(storage::decode_cursor) {
        Some(None) => {
            return ToolResult::error("cursor is malformed; pass next_cursor from a previous call")
        }
        Some(Some(c)) => Some(c),
        None => None,
    };

    let filter = RunListFilter {
        visibility: vis.clone(),
        project: args.project,
        suite: args.suite,
        tag: args.tag,
        branch: args.branch,
        since_ms,
        until_ms,
        status: args.status,
        cached: args.cached,
        origin: args.origin,
        actor: args.actor,
        limit: clamp_limit(args.limit, FIND_DEFAULT_LIMIT, FIND_MAX_LIMIT),
        cursor,
    };

    let page = match state.storage.list_runs(filter).await {
        Ok(page) => page,
        Err(e) => return internal(e, "listing runs"),
    };

    let mut structured = json!({
        "runs": page.runs,
        "next_cursor": page.next_cursor,
        "cached_hidden": page.cached_hidden,
    });
    let text = text::runs_table(&structured["runs"]);
    finish(structured_with_budget(&mut structured), structured, text)
}

async fn list_projects(state: &AppState, vis: &RunVisibility) -> ToolResult {
    match state.storage.list_projects(vis.clone()).await {
        Ok(projects) => {
            let structured = json!(projects);
            let text = text::projects_table(&structured["projects"]);
            ToolResult::ok(structured, text)
        }
        Err(e) => internal(e, "listing projects"),
    }
}

async fn list_suites(state: &AppState, vis: &RunVisibility, project: String) -> ToolResult {
    match state
        .storage
        .list_suites(project.clone(), vis.clone())
        .await
    {
        Ok(suites) => {
            let structured = json!(suites);
            let text = text::suites_table(&project, &structured["suites"]);
            ToolResult::ok(structured, text)
        }
        Err(e) => internal(e, "listing suites"),
    }
}

pub(super) async fn get_run(state: &AppState, vis: &RunVisibility, args: Value) -> ToolResult {
    let args: GetRunArgs = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let run_id = RunId::new(args.run_id.clone());

    let detail = match state.storage.get_run(run_id.clone(), vis.clone()).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            return ToolResult::error(format!(
                "no run '{}'. Use find_runs to list run ids.",
                args.run_id
            ))
        }
        Err(e) => return internal(e, "loading run"),
    };

    let mut structured = json!({ "run": detail });
    let include = args.include.unwrap_or_default();

    if include.contains(&RunInclude::Matrix) {
        let filter = MatrixFilter {
            run_id: run_id.clone(),
            visibility: vis.clone(),
            limit: clamp_limit(args.matrix_limit, MATRIX_DEFAULT_LIMIT, MATRIX_MAX_LIMIT),
            cursor: args.matrix_cursor,
        };
        match state.storage.run_matrix(filter).await {
            Ok(matrix) => structured["matrix"] = json!(matrix),
            Err(e) => return internal(e, "loading matrix"),
        }
    }

    if include.contains(&RunInclude::Config) {
        match state.storage.get_run_config(run_id, vis.clone()).await {
            Ok(Some(config)) => structured["config"] = json!(config),
            // A run always has a detail row; its config snapshot may predate
            // config capture. Absence is informative, not an error.
            Ok(None) => structured["config"] = Value::Null,
            Err(e) => return internal(e, "loading run config"),
        }
    }

    let text = text::run_summary(&structured);
    finish(structured_with_budget(&mut structured), structured, text)
}

// -- shared helpers ----------------------------------------------------------

fn parse_time(raw: Option<&str>, field: &str) -> Result<Option<i64>, ToolResult> {
    match raw {
        None => Ok(None),
        Some(raw) => storage::parse_time_ms(raw).map(Some).ok_or_else(|| {
            ToolResult::error(format!(
                "{field} '{raw}' is not a timestamp; use RFC3339 (2026-07-28T00:00:00Z) or epoch milliseconds"
            ))
        }),
    }
}

/// Sanitize and truncate a payload, returning the truncation record so the
/// caller can attach it after the text form is built.
pub(super) fn structured_with_budget(structured: &mut Value) -> Budget {
    let mut budget = Budget::new(crate::mcp::budget::MAX_STRING);
    budget.apply(structured);
    budget
}

/// Attach truncation metadata and enforce the response ceiling.
pub(super) fn finish(budget: Budget, mut structured: Value, text: String) -> ToolResult {
    budget.annotate(&mut structured);
    if !crate::mcp::budget::fits(&structured) {
        return ToolResult::error(
            "result exceeded the response budget. Narrow the filters or lower `limit` and retry.",
        );
    }
    ToolResult::ok(structured, text)
}

pub(super) fn internal(error: anyhow::Error, what: &str) -> ToolResult {
    tracing::error!(error = %error, "mcp tool failed while {what}");
    ToolResult::error(format!("internal error while {what}"))
}
