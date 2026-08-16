//! Resolving the `--against` baseline for a run.
//!
//! Separate from [`crate::loadrun`], which only knows how to find and read a
//! stored run. This module holds the *policy*: which run a regression gate
//! should be compared against, and — just as importantly — which failures to
//! resolve one are fatal.

use domarinn_core::RunResult;
use serde::Deserialize;

use crate::loadrun;

/// The reference that resolves to the server's pinned baseline for this suite.
///
/// Namespaced with a `server:` prefix so it can never collide with a run id or a
/// relative path, both of which `--against` also accepts.
pub const SERVER_BASELINE: &str = "server:baseline";

/// The pinless server branch reference: `server:branch:<name>` merges the
/// newest server runs on `<name>` into a composite baseline, no pin required.
const SERVER_BRANCH: &str = "server:branch:";

/// The local branch reference: `branch:<name>` merges the newest runs on
/// `<name>` from the local store.
const LOCAL_BRANCH: &str = "branch:";

/// The explicit opt-out. Handled by the caller before [`resolve`] — a suite
/// config can make a comparison the default, and the flag must be able to turn
/// it off.
pub const NONE: &str = "none";

/// The machine codes the export endpoint stamps on "nothing to compare
/// against" 404s. A recognized code is an [`BaselineError::Absent`]; an
/// unrecognized one is NOT — a future coded *fatal* error must not be waved
/// through as a first run.
const ABSENT_CODES: &[&str] = &["baseline_unpinned", "no_runs_on_branch", "unknown_suite"];

/// The reference actually in force: the flag when given (`none` disabling
/// outright), else the suite's `baseline.branch` default — aimed at the server
/// when one is configured (a fresh CI checkout has no local store, which is
/// the whole reason to have a default) and at the local store otherwise.
pub fn effective(
    flag: Option<&str>,
    suite_branch: Option<&str>,
    server_configured: bool,
) -> Option<String> {
    match flag {
        Some(NONE) => None,
        Some(explicit) => Some(explicit.to_string()),
        None => suite_branch.map(|branch| {
            if server_configured {
                format!("{SERVER_BRANCH}{branch}")
            } else {
                format!("{LOCAL_BRANCH}{branch}")
            }
        }),
    }
}

/// Whether a results server is reachable in principle — the same derivation
/// [`resolve`]'s server arms use (the global `--server-url`, else
/// `DOMARINN_SERVER_URL`).
pub fn server_configured(server_url: Option<&str>) -> bool {
    server_url.map(|s| !s.is_empty()).unwrap_or(false)
        || std::env::var("DOMARINN_SERVER_URL")
            .map(|s| !s.is_empty())
            .unwrap_or(false)
}

/// Why a requested baseline could not be produced.
///
/// The two-way split is the entire point of the type. Treating every failure as
/// "no baseline, carry on" is what made the documented CI gate a no-op: a fresh
/// checkout has no local run store, so `--against latest` resolved to nothing,
/// the comparison was skipped, `regressed` stayed false, and the job exited 0 on
/// a real regression.
#[derive(Debug)]
pub enum BaselineError {
    /// No baseline exists yet. The normal state on a suite's first run against a
    /// fresh server, and the only case a caller may reasonably continue past.
    Absent(String),
    /// A baseline was requested and should have been resolvable. Always fatal:
    /// continuing would report a green run that was never actually compared.
    Failed(String),
}

/// Resolve `reference` to the run this one should be compared against.
pub fn resolve(
    reference: &str,
    head: &RunResult,
    server_url: Option<&str>,
) -> Result<RunResult, BaselineError> {
    match reference {
        SERVER_BASELINE => from_server(head, server_url, None),
        r if r.starts_with(SERVER_BRANCH) => {
            let branch = &r[SERVER_BRANCH.len()..];
            if branch.is_empty() {
                return Err(BaselineError::Failed(
                    "`--against server:branch:` names no branch".to_string(),
                ));
            }
            from_server(head, server_url, Some(branch))
        }
        // Reserve the whole namespace: a typo like `server:latest` must not
        // fall through to the local-run-id arm and error confusingly there.
        r if r.starts_with("server:") => Err(BaselineError::Failed(format!(
            "unknown server reference '{r}'; expected `server:baseline` or `server:branch:<name>`"
        ))),
        r if r.starts_with(LOCAL_BRANCH) => {
            let branch = &r[LOCAL_BRANCH.len()..];
            if branch.is_empty() {
                return Err(BaselineError::Failed(
                    "`--against branch:` names no branch".to_string(),
                ));
            }
            let runs = loadrun::runs_on_branch(head, branch);
            domarinn_core::composite::merge_branch_runs(branch, runs).ok_or_else(|| {
                BaselineError::Absent(format!(
                    "no earlier local runs of {} on branch {branch} to merge into a baseline",
                    label(head)
                ))
            })
        }
        "latest" => {
            let path = loadrun::latest_for_suite(head).ok_or_else(|| {
                BaselineError::Absent(format!(
                    "no earlier local run of {} to compare against",
                    label(head)
                ))
            })?;
            let text = std::fs::read_to_string(&path)
                .map_err(|e| BaselineError::Failed(format!("reading {}: {e}", path.display())))?;
            serde_json::from_str(&text)
                .map_err(|e| BaselineError::Failed(format!("parsing {}: {e}", path.display())))
        }
        // An explicitly named run that cannot be loaded is a usage error, not an
        // absence — the user asked for a specific thing that is not there.
        explicit => {
            let base = loadrun::load_run(explicit).map_err(BaselineError::Failed)?;
            same_suite(&base, head)?;
            Ok(base)
        }
    }
}

/// Reject a cross-suite comparison. `diff_runs` joins on `case_key`, which is
/// derived from ids alone and carries no suite, so comparing two suites produces
/// a plausible-looking diff of entirely unrelated cases rather than an obvious
/// mismatch.
fn same_suite(base: &RunResult, head: &RunResult) -> Result<(), BaselineError> {
    if base.project == head.project && base.suite == head.suite {
        return Ok(());
    }
    Err(BaselineError::Failed(format!(
        "baseline {} is {}, but this run is {} — comparing different suites is never meaningful",
        base.run_id.as_str(),
        label(base),
        label(head)
    )))
}

/// A human label for a run's suite, for error messages.
fn label(run: &RunResult) -> String {
    match (run.project.as_deref(), run.suite.as_deref()) {
        (Some(p), Some(s)) => format!("{p}/{s}"),
        (None, Some(s)) => s.to_string(),
        (Some(p), None) => format!("{p}/(unnamed suite)"),
        (None, None) => "(unnamed suite)".to_string(),
    }
}

fn from_server(
    head: &RunResult,
    server_url: Option<&str>,
    branch: Option<&str>,
) -> Result<RunResult, BaselineError> {
    let server = server_url
        .map(String::from)
        .or_else(|| std::env::var("DOMARINN_SERVER_URL").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            BaselineError::Failed(
                "`--against server:...` needs a server (set --server-url or DOMARINN_SERVER_URL)"
                    .to_string(),
            )
        })?;

    // The server keys baselines and runs on (project, suite), so a run that
    // names neither cannot address one. Say so rather than guessing at a
    // default that would silently pin the wrong suite.
    let (Some(project), Some(suite)) = (head.project.as_deref(), head.suite.as_deref()) else {
        return Err(BaselineError::Failed(
            "`--against server:...` needs both `project:` and `suite:` set in the suite \
             config; the server keys baselines on (project, suite)"
                .to_string(),
        ));
    };

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| BaselineError::Failed(format!("starting runtime: {e}")))?;
    runtime.block_on(fetch_export(&server, project, suite, head, branch))
}

/// Resolve through `GET .../baseline/export` — one request for every server
/// reference form, with the head run excluded from any composite server-side.
///
/// The 404 protocol is the load-bearing part. A 404 whose body carries a
/// recognized machine `code` is an absence (nothing pinned, no runs on the
/// branch yet) — the caller may continue. A *bare* 404 means the server
/// predates the route: for `server:baseline` the pin may still exist and be
/// readable the old way, so resolution falls back to [`fetch_legacy`]; for a
/// branch reference the old wire cannot express the question at all, and
/// skipping the gate would be the exact silent no-op this module exists to
/// prevent — so it is fatal.
async fn fetch_export(
    server: &str,
    project: &str,
    suite: &str,
    head: &RunResult,
    branch: Option<&str>,
) -> Result<RunResult, BaselineError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| BaselineError::Failed(e.to_string()))?;

    let mut export_url = url(
        server,
        &[
            "api", "v1", "projects", project, "suites", suite, "baseline", "export",
        ],
    )?;
    {
        let mut query = export_url.query_pairs_mut();
        if let Some(branch) = branch {
            query.append_pair("branch", branch);
        }
        query.append_pair("exclude", head.run_id.as_str());
    }

    let (status, body) = get_raw(&client, &export_url).await?;
    if status == reqwest::StatusCode::NOT_FOUND {
        let parsed: Option<serde_json::Value> = serde_json::from_str(&body).ok();
        let code = parsed
            .as_ref()
            .and_then(|v| v.get("code")?.as_str())
            .map(str::to_string);
        return match code {
            Some(code) if ABSENT_CODES.contains(&code.as_str()) => {
                let message = parsed
                    .as_ref()
                    .and_then(|v| v.get("error")?.as_str())
                    .unwrap_or(code.as_str())
                    .to_string();
                Err(BaselineError::Absent(message))
            }
            // A coded 404 this build does not recognize: refuse rather than
            // wave a future fatal condition through as a first run.
            Some(code) => Err(BaselineError::Failed(format!(
                "GET {export_url}: HTTP 404 ({code}): {body}"
            ))),
            None => match branch {
                None => fetch_legacy(&client, server, project, suite, head).await,
                Some(branch) => Err(BaselineError::Failed(format!(
                    "the server at {server} does not support branch baseline references \
                     (`server:branch:{branch}` needs GET .../baseline/export); upgrade the \
                     server, or pin a baseline and use `server:baseline`"
                ))),
            },
        };
    }
    if !status.is_success() {
        return Err(BaselineError::Failed(format!(
            "GET {export_url}: HTTP {}: {body}",
            status.as_u16()
        )));
    }

    let base: RunResult = serde_json::from_str(&body)
        .map_err(|e| BaselineError::Failed(format!("parsing response from {export_url}: {e}")))?;
    // A run pin pointing at the run being gated compares it to itself and
    // reports a permanent all-clear. (A composite can never collide: its id is
    // synthetic.)
    if base.run_id == head.run_id {
        return Err(BaselineError::Absent(format!(
            "the baseline pinned for {project}/{suite} is this run itself"
        )));
    }
    same_suite(&base, head)?;
    Ok(base)
}

/// One suite row of `GET /projects/{project}/suites`, projected to the two
/// fields this needs.
#[derive(Deserialize)]
struct SuiteRow {
    suite: String,
    baseline_run_id: Option<String>,
}

#[derive(Deserialize)]
struct SuitesBody {
    suites: Vec<SuiteRow>,
}

/// The pre-export-endpoint resolution: read `baseline_run_id` off the suites
/// listing, then export that run. Kept verbatim so `server:baseline` against an
/// older server behaves exactly as it did before the export endpoint existed.
async fn fetch_legacy(
    client: &reqwest::Client,
    server: &str,
    project: &str,
    suite: &str,
    head: &RunResult,
) -> Result<RunResult, BaselineError> {
    let suites: SuitesBody = get(
        client,
        &url(server, &["api", "v1", "projects", project, "suites"])?,
    )
    .await?;

    let row = suites
        .suites
        .iter()
        .find(|s| s.suite == suite)
        .ok_or_else(|| {
            BaselineError::Absent(format!("server knows no suite {project}/{suite} yet"))
        })?;
    let baseline_id = row.baseline_run_id.as_deref().ok_or_else(|| {
        BaselineError::Absent(format!(
            "no baseline pinned for {project}/{suite} — pin one in the web UI, or on a run's page"
        ))
    })?;

    // Pinning the run that is currently being gated would compare it to itself
    // and report a permanent all-clear.
    if baseline_id == head.run_id.as_str() {
        return Err(BaselineError::Absent(format!(
            "the baseline pinned for {project}/{suite} is this run itself"
        )));
    }

    get(
        client,
        &url(server, &["api", "v1", "runs", baseline_id, "export"])?,
    )
    .await
}

/// Build a URL from path segments. `path_segments_mut` percent-encodes each
/// segment, so a project or suite containing `/` or a space cannot break out of
/// its position in the path.
fn url(server: &str, segments: &[&str]) -> Result<reqwest::Url, BaselineError> {
    let mut url = reqwest::Url::parse(server.trim_end_matches('/'))
        .map_err(|e| BaselineError::Failed(format!("bad server URL '{server}': {e}")))?;
    url.path_segments_mut()
        .map_err(|_| BaselineError::Failed(format!("server URL '{server}' cannot have a path")))?
        .extend(segments);
    Ok(url)
}

/// One authenticated GET, returning the status and body for the caller to
/// interpret — [`fetch_export`]'s 404 protocol needs both.
async fn get_raw(
    client: &reqwest::Client,
    url: &reqwest::Url,
) -> Result<(reqwest::StatusCode, String), BaselineError> {
    let mut request = client.get(url.clone());
    if let Ok(token) = std::env::var("DOMARINN_TOKEN") {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response = request
        .send()
        .await
        .map_err(|e| BaselineError::Failed(format!("GET {url}: {e}")))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

async fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &reqwest::Url,
) -> Result<T, BaselineError> {
    let (status, body) = get_raw(client, url).await?;
    if !status.is_success() {
        return Err(BaselineError::Failed(format!(
            "GET {url}: HTTP {}: {body}",
            status.as_u16()
        )));
    }
    serde_json::from_str(&body)
        .map_err(|e| BaselineError::Failed(format!("parsing response from {url}: {e}")))
}
