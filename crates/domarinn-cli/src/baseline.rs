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
        SERVER_BASELINE => from_server(head, server_url),
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

fn from_server(head: &RunResult, server_url: Option<&str>) -> Result<RunResult, BaselineError> {
    let server = server_url
        .map(String::from)
        .or_else(|| std::env::var("DOMARINN_SERVER_URL").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            BaselineError::Failed(
                "`--against server:baseline` needs a server (set --server-url or \
                 DOMARINN_SERVER_URL)"
                    .to_string(),
            )
        })?;

    // The server pins baselines per (project, suite) — both columns are NOT NULL
    // — so a run that names neither cannot address one. Say so rather than
    // guessing at a default that would silently pin the wrong suite.
    let (Some(project), Some(suite)) = (head.project.as_deref(), head.suite.as_deref()) else {
        return Err(BaselineError::Failed(
            "`--against server:baseline` needs both `project:` and `suite:` set in the suite \
             config; the server pins one baseline per (project, suite)"
                .to_string(),
        ));
    };

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| BaselineError::Failed(format!("starting runtime: {e}")))?;
    runtime.block_on(fetch(&server, project, suite, head))
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

async fn fetch(
    server: &str,
    project: &str,
    suite: &str,
    head: &RunResult,
) -> Result<RunResult, BaselineError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| BaselineError::Failed(e.to_string()))?;

    let suites: SuitesBody = get(
        &client,
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
        &client,
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

async fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &reqwest::Url,
) -> Result<T, BaselineError> {
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
    if !status.is_success() {
        return Err(BaselineError::Failed(format!(
            "GET {url}: HTTP {}: {body}",
            status.as_u16()
        )));
    }
    serde_json::from_str(&body)
        .map_err(|e| BaselineError::Failed(format!("parsing response from {url}: {e}")))
}
