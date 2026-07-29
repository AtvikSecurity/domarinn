//! The `domarinn runs` command: discover past runs without knowing their ids.
//!
//! By default it lists the local `.domarinn/runs` store (newest first); with
//! `--remote` it queries the results server's `GET /api/v1/runs`. The human
//! table is the only palette-aware output — `--json` is never colored, and
//! `--remote --json` is a verbatim pass-through of the server response body so
//! it stays forward-compatible with server-side fields this CLI predates.
//!
//! The `--json` (local) shape is an array of objects:
//! `{run_id, project, suite, started_at, finished_at, summary, git, path,
//! latest}` — `summary`/`git` are the run document's own serde representations,
//! `path` is the on-disk `result.json`, and `latest` flags the `latest` pointer
//! target.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use clap::Args;
use domarinn_core::result::{GitMeta, RunSummary};
use serde::{Deserialize, Serialize};

use crate::exit;
use crate::output::{humanize_duration, truncate};
use crate::style::Palette;

#[derive(Args)]
pub struct RunsArgs {
    /// Max runs to show (newest first); 0 = unlimited.
    #[arg(short = 'n', long, default_value_t = 20)]
    pub limit: usize,
    /// Only runs of this suite.
    #[arg(long)]
    pub suite: Option<String>,
    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
    /// List runs from the results server instead of .domarinn/runs.
    #[arg(long)]
    pub remote: bool,
}

pub fn execute(args: RunsArgs, server_url: Option<String>, palette: Palette) -> u8 {
    if args.remote {
        execute_remote(&args, server_url, &palette)
    } else {
        execute_local(&args, &palette)
    }
}

// ---------------------------------------------------------------------------
// Local listing
// ---------------------------------------------------------------------------

/// A lean projection of a stored `result.json`. Since v2 the `cases` array can
/// be large, so it (and any other field) is simply omitted here: serde ignores
/// keys with no matching struct field, so this deserializes the whole document
/// but only materializes what the listing needs.
#[derive(Deserialize)]
struct LeanRun {
    run_id: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    suite: Option<String>,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    summary: RunSummary,
    #[serde(default)]
    git: Option<GitMeta>,
}

/// A run discovered on disk, paired with the `result.json` path it came from.
struct LocalEntry {
    run: LeanRun,
    path: PathBuf,
}

fn execute_local(args: &RunsArgs, palette: &Palette) -> u8 {
    let dir = crate::loadrun::runs_dir();
    let latest_id = std::fs::read_to_string(crate::loadrun::latest_pointer())
        .ok()
        .map(|s| s.trim().to_string());

    let entries = scan_runs(&dir);
    let discovered = entries.len();
    let (entries, matched) = arrange(entries, args.suite.as_deref(), args.limit);

    if args.json {
        let rows = local_json(&entries, latest_id.as_deref());
        return match serde_json::to_string_pretty(&rows) {
            Ok(s) => {
                println!("{s}");
                exit::OK
            }
            Err(e) => {
                eprintln!("error: serializing runs: {e}");
                exit::INFRA
            }
        };
    }

    if discovered == 0 {
        println!("no runs found in .domarinn/runs (run a suite first, or try --remote)");
        return exit::OK;
    }
    if matched == 0 {
        match &args.suite {
            Some(s) => println!("no runs match suite '{s}' ({discovered} run(s) on disk)"),
            None => println!("no runs found in .domarinn/runs"),
        }
        return exit::OK;
    }

    let rows = to_local_rows(&entries, latest_id.as_deref());
    let footer = format!("{matched} run{} in .domarinn/runs", plural(matched));
    print!("{}", render_table(&rows, palette, &footer));
    exit::OK
}

/// Read every `<run_id>/result.json` under `dir`, skipping (with a warning)
/// anything unreadable or corrupt. A missing directory yields no runs.
fn scan_runs(dir: &Path) -> Vec<LocalEntry> {
    let mut entries = Vec::new();
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return entries,
    };
    for dent in read_dir.flatten() {
        let path = dent.path();
        if !path.is_dir() {
            continue;
        }
        let result_path = path.join("result.json");
        if !result_path.is_file() {
            continue;
        }
        match load_lean(&result_path) {
            Ok(run) => entries.push(LocalEntry {
                run,
                path: result_path,
            }),
            Err(e) => {
                tracing::warn!(path = %result_path.display(), error = %e, "skipping unreadable run");
            }
        }
    }
    entries
}

fn load_lean(path: &Path) -> Result<LeanRun, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Sort newest-first, apply the optional `--suite` filter, then cap to `limit`
/// (0 = unlimited). Returns the (possibly truncated) entries plus how many
/// matched the filter *before* the limit, so the caller can report the true
/// total even when the display is capped.
fn arrange(
    mut entries: Vec<LocalEntry>,
    suite: Option<&str>,
    limit: usize,
) -> (Vec<LocalEntry>, usize) {
    entries.sort_by_key(|e| std::cmp::Reverse(e.run.started_at));
    if let Some(s) = suite {
        entries.retain(|e| e.run.suite.as_deref() == Some(s));
    }
    let matched = entries.len();
    if limit > 0 && entries.len() > limit {
        entries.truncate(limit);
    }
    (entries, matched)
}

/// One entry in `runs --json`. `summary`/`git` borrow the run document's own
/// serde representations verbatim; `path` and `latest` are the listing's
/// additions.
#[derive(Serialize)]
struct LocalRunJson<'a> {
    run_id: &'a str,
    project: &'a Option<String>,
    suite: &'a Option<String>,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    summary: &'a RunSummary,
    git: &'a Option<GitMeta>,
    path: String,
    latest: bool,
}

fn local_json<'a>(entries: &'a [LocalEntry], latest_id: Option<&str>) -> Vec<LocalRunJson<'a>> {
    entries
        .iter()
        .map(|e| LocalRunJson {
            run_id: e.run.run_id.as_str(),
            project: &e.run.project,
            suite: &e.run.suite,
            started_at: e.run.started_at,
            finished_at: e.run.finished_at,
            summary: &e.run.summary,
            git: &e.run.git,
            path: e.path.display().to_string(),
            latest: latest_id == Some(e.run.run_id.as_str()),
        })
        .collect()
}

fn to_local_rows(entries: &[LocalEntry], latest_id: Option<&str>) -> Vec<RunRow> {
    entries
        .iter()
        .map(|e| {
            let s = &e.run.summary;
            RunRow {
                marker: if latest_id == Some(e.run.run_id.as_str()) {
                    '*'
                } else {
                    ' '
                },
                id: e.run.run_id.clone(),
                when: format_local_time(e.run.started_at),
                suite: truncate(e.run.suite.as_deref().unwrap_or("-"), 16),
                score: format!("{}/{}", s.passed, s.total),
                sev: severity(s.failed, s.errored),
                duration: humanize_duration(duration_secs(e.run.started_at, e.run.finished_at)),
                cost: format_cost(s.cost_usd),
                git: format_git(e.run.git.as_ref()),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Remote listing
// ---------------------------------------------------------------------------

/// A local, deserialize-only mirror of the server's `RunListResponse`. The
/// server DTO derives `Serialize` only, so we cannot reuse it here; this mirror
/// carries just the fields the table renders (the `--remote --json` path bypasses
/// it entirely, printing the raw body).
#[derive(Deserialize)]
struct RemoteResponse {
    runs: Vec<RemoteRun>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct RemoteRun {
    id: String,
    #[serde(default)]
    suite: Option<String>,
    /// RFC3339.
    created_at: String,
    #[serde(default)]
    git_branch: Option<String>,
    #[serde(default)]
    git_commit: Option<String>,
    #[serde(default)]
    git_dirty: Option<bool>,
    #[serde(default)]
    case_count: i64,
    #[serde(default)]
    pass_count: i64,
    #[serde(default)]
    fail_count: i64,
    #[serde(default)]
    error_count: i64,
    #[serde(default)]
    cost_usd: Option<f64>,
    #[serde(default)]
    duration_ms: i64,
}

fn execute_remote(args: &RunsArgs, server_url: Option<String>, palette: &Palette) -> u8 {
    let server = match resolve_server(server_url) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let body = match fetch_remote(&server, args) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::INFRA;
        }
    };

    if args.json {
        // Verbatim pass-through: forward-compatible with server fields this CLI
        // does not model. Never colored.
        print!("{body}");
        if !body.ends_with('\n') {
            println!();
        }
        return exit::OK;
    }

    let resp: RemoteResponse = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: parsing server response: {e}");
            return exit::INFRA;
        }
    };
    if resp.runs.is_empty() {
        println!("no runs on {server}");
        return exit::OK;
    }
    let rows = to_remote_rows(&resp.runs);
    let mut footer = format!(
        "{} run{} from {server}",
        resp.runs.len(),
        plural(resp.runs.len())
    );
    if resp.next_cursor.is_some() {
        footer.push_str(" (more available; raise -n)");
    }
    print!("{}", render_table(&rows, palette, &footer));
    exit::OK
}

/// Resolve the server base URL from `--server-url` then `DOMARINN_SERVER_URL`,
/// matching `share.rs`. Absence is a usage error (exit 2), not an infra one.
fn resolve_server(server_url: Option<String>) -> Result<String, String> {
    server_url
        .or_else(|| std::env::var("DOMARINN_SERVER_URL").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "no server URL (set --server-url or DOMARINN_SERVER_URL)".to_string())
}

/// GET `{server}/api/v1/runs?suite=&limit=`, following the `share.rs`
/// conventions: a 60s timeout, an optional `DOMARINN_TOKEN` bearer, and its own
/// short-lived tokio runtime. Returns the raw response body on 2xx, an error
/// (transport or non-2xx) otherwise.
fn fetch_remote(server: &str, args: &RunsArgs) -> Result<String, String> {
    let mut url = reqwest::Url::parse(&format!("{}/api/v1/runs", server.trim_end_matches('/')))
        .map_err(|e| format!("invalid server URL: {e}"))?;
    {
        let mut qp = url.query_pairs_mut();
        if let Some(suite) = &args.suite {
            qp.append_pair("suite", suite);
        }
        if args.limit > 0 {
            qp.append_pair("limit", &args.limit.to_string());
        }
    }

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| e.to_string())?;
        let mut request = client.get(url);
        if let Ok(token) = std::env::var("DOMARINN_TOKEN") {
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("HTTP {}: {text}", status.as_u16()));
        }
        Ok(text)
    })
}

fn to_remote_rows(runs: &[RemoteRun]) -> Vec<RunRow> {
    runs.iter()
        .map(|r| {
            let passed = r.pass_count.max(0) as u64;
            let total = r.case_count.max(0) as u64;
            let failed = r.fail_count.max(0) as u64;
            let errored = r.error_count.max(0) as u64;
            RunRow {
                marker: ' ',
                id: r.id.clone(),
                when: format_rfc3339_local(&r.created_at),
                suite: truncate(r.suite.as_deref().unwrap_or("-"), 16),
                score: format!("{passed}/{total}"),
                sev: severity(failed, errored),
                duration: humanize_duration((r.duration_ms.max(0) as f64) / 1000.0),
                cost: format_cost(r.cost_usd),
                git: format_git_parts(
                    r.git_branch.as_deref(),
                    r.git_commit.as_deref(),
                    r.git_dirty.unwrap_or(false),
                ),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Shared row model + renderer
// ---------------------------------------------------------------------------

/// The color severity of a run's `passed/total` cell. Infra errors outrank
/// assertion failures — the same precedence the exit codes use (3 beats 1),
/// because an errored run's numbers cannot be trusted, so that is the louder
/// signal to surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sev {
    Fail,
    Error,
}

fn severity(failed: u64, errored: u64) -> Option<Sev> {
    if errored > 0 {
        Some(Sev::Error)
    } else if failed > 0 {
        Some(Sev::Fail)
    } else {
        None
    }
}

/// A rendered row, shared by the local and remote listings so both tables look
/// identical. All display strings are precomputed; `sev` drives the (optional)
/// color of the `score` cell at render time.
struct RunRow {
    marker: char,
    id: String,
    when: String,
    suite: String,
    score: String,
    sev: Option<Sev>,
    duration: String,
    cost: String,
    git: String,
}

/// Render the rows as a hand-rolled fixed-width table (the `output.rs` idiom):
/// per-column widths are the max of the cells and the header, and the colored
/// `score` cell is padded manually so ANSI escapes never distort alignment. The
/// `cost` column is omitted entirely when no row has a cost.
fn render_table(rows: &[RunRow], palette: &Palette, footer: &str) -> String {
    let id_w = rows
        .iter()
        .map(|r| r.id.chars().count())
        .max()
        .unwrap_or(0)
        .max(6); // "run id"
    let when_w = rows
        .iter()
        .map(|r| r.when.chars().count())
        .max()
        .unwrap_or(0)
        .max(7); // "started"
    let suite_w = rows
        .iter()
        .map(|r| r.suite.chars().count())
        .max()
        .unwrap_or(0)
        .max(5); // "suite"
    let score_w = rows
        .iter()
        .map(|r| r.score.chars().count())
        .max()
        .unwrap_or(0)
        .max(4); // "pass"
    let dur_w = rows
        .iter()
        .map(|r| r.duration.chars().count())
        .max()
        .unwrap_or(0)
        .max(3); // "dur"
    let any_cost = rows.iter().any(|r| !r.cost.is_empty());
    let cost_w = if any_cost {
        rows.iter()
            .map(|r| r.cost.chars().count())
            .max()
            .unwrap_or(0)
            .max(4) // "cost"
    } else {
        0
    };

    let mut out = String::new();

    let mut header = format!(
        "  {:<id_w$}  {:<when_w$}  {:<suite_w$}  {:<score_w$}  {:<dur_w$}",
        "run id", "started", "suite", "pass", "dur",
    );
    if any_cost {
        header.push_str(&format!("  {:<cost_w$}", "cost"));
    }
    header.push_str("  git");
    out.push_str(&palette.header(header.trim_end()));
    out.push('\n');

    for r in rows {
        let score_pad = score_w.saturating_sub(r.score.chars().count());
        let score = match r.sev {
            Some(Sev::Error) => palette.error(&r.score),
            Some(Sev::Fail) => palette.fail(&r.score),
            None => r.score.clone(),
        };
        let mut line = format!(
            "{} {:<id_w$}  {:<when_w$}  {:<suite_w$}  {score}{:score_pad$}  {:<dur_w$}",
            r.marker, r.id, r.when, r.suite, "", r.duration,
        );
        if any_cost {
            line.push_str(&format!("  {:<cost_w$}", r.cost));
        }
        if !r.git.is_empty() {
            line.push_str(&format!("  {}", r.git));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }

    out.push('\n');
    out.push_str(footer);
    out.push('\n');
    out
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Wall-clock run duration in seconds, clamped at zero to absorb clock skew
/// between `started_at` and `finished_at`.
fn duration_secs(started: DateTime<Utc>, finished: DateTime<Utc>) -> f64 {
    ((finished - started).num_milliseconds() as f64 / 1000.0).max(0.0)
}

/// A UTC instant rendered in the local timezone as `YYYY-MM-DD HH:MM`.
fn format_local_time(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// An RFC3339 timestamp rendered in the local timezone like [`format_local_time`],
/// falling back to the raw string if it does not parse.
fn format_rfc3339_local(s: &str) -> String {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| s.to_string())
}

/// `$0.1234` when a positive cost is present, else empty.
fn format_cost(cost: Option<f64>) -> String {
    cost.filter(|c| *c > 0.0)
        .map(|c| format!("${c:.4}"))
        .unwrap_or_default()
}

fn format_git(git: Option<&GitMeta>) -> String {
    match git {
        Some(g) => format_git_parts(g.branch.as_deref(), g.commit.as_deref(), g.dirty),
        None => String::new(),
    }
}

/// `branch@commit7` (short 7-char commit), with a trailing `+` when the tree was
/// dirty. Degrades gracefully when only one of branch/commit is known, and is
/// empty when neither is.
fn format_git_parts(branch: Option<&str>, commit: Option<&str>, dirty: bool) -> String {
    let branch = branch.unwrap_or("").trim();
    let commit7: String = commit.unwrap_or("").chars().take(7).collect();
    let mut s = match (branch.is_empty(), commit7.is_empty()) {
        (true, true) => return String::new(),
        (false, true) => branch.to_string(),
        (true, false) => commit7,
        (false, false) => format!("{branch}@{commit7}"),
    };
    if dirty {
        s.push('+');
    }
    s
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full `result.json` (v2, with a populated `cases` array) — the listing
    /// must deserialize its lean projection and silently ignore `cases`.
    const FULL_RESULT_JSON: &str = r#"{
        "schema_version": 2,
        "run_id": "01JULY0000000000000000RUN",
        "project": "proj",
        "suite": "mysuite",
        "started_at": "2026-01-01T00:00:00Z",
        "finished_at": "2026-01-01T00:00:30Z",
        "config_digest": "d",
        "config_snapshot": {"k": "v"},
        "git": {"branch": "main", "commit": "deadbeefcafe1234", "dirty": true},
        "cases": [
            {"cell": {"provider_id": "p", "test_id": "t"}, "case_key": "abc0000000000000", "status": "pass", "score": 1.0, "latency_ms": 5},
            {"cell": {"provider_id": "p", "test_id": "u"}, "case_key": "def0000000000000", "status": "fail", "score": 0.0, "latency_ms": 7}
        ],
        "summary": {"total": 2, "passed": 1, "failed": 1, "errored": 0, "skipped": 0, "cost_usd": 0.0025}
    }"#;

    #[test]
    fn lean_parse_reads_summary_and_git_and_ignores_cases() {
        let lean: LeanRun = serde_json::from_str(FULL_RESULT_JSON).unwrap();
        assert_eq!(lean.run_id, "01JULY0000000000000000RUN");
        assert_eq!(lean.project.as_deref(), Some("proj"));
        assert_eq!(lean.suite.as_deref(), Some("mysuite"));
        assert_eq!(lean.summary.total, 2);
        assert_eq!(lean.summary.passed, 1);
        assert_eq!(lean.summary.failed, 1);
        assert_eq!(lean.summary.cost_usd, Some(0.0025));
        let git = lean.git.unwrap();
        assert_eq!(git.branch.as_deref(), Some("main"));
        assert!(git.dirty);
        // `cases` was present but has no struct field, so it is dropped — the
        // point of the lean projection.
    }

    fn entry(id: &str, suite: Option<&str>, secs: i64, summary: RunSummary) -> LocalEntry {
        LocalEntry {
            run: LeanRun {
                run_id: id.to_string(),
                project: None,
                suite: suite.map(str::to_string),
                started_at: DateTime::from_timestamp(secs, 0).unwrap(),
                finished_at: DateTime::from_timestamp(secs + 10, 0).unwrap(),
                summary,
                git: None,
            },
            path: PathBuf::from(format!(".domarinn/runs/{id}/result.json")),
        }
    }

    fn ids(entries: &[LocalEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.run.run_id.as_str()).collect()
    }

    #[test]
    fn arrange_sorts_newest_first() {
        let entries = vec![
            entry("old", None, 100, RunSummary::default()),
            entry("new", None, 300, RunSummary::default()),
            entry("mid", None, 200, RunSummary::default()),
        ];
        let (out, matched) = arrange(entries, None, 0);
        assert_eq!(ids(&out), ["new", "mid", "old"]);
        assert_eq!(matched, 3);
    }

    #[test]
    fn arrange_limit_zero_is_all_and_positive_truncates_after_sort() {
        let mk = || {
            vec![
                entry("a", None, 100, RunSummary::default()),
                entry("b", None, 200, RunSummary::default()),
                entry("c", None, 300, RunSummary::default()),
            ]
        };
        let (all, matched) = arrange(mk(), None, 0);
        assert_eq!(all.len(), 3);
        assert_eq!(matched, 3);

        let (one, matched) = arrange(mk(), None, 1);
        assert_eq!(ids(&one), ["c"], "keeps the newest after the sort");
        assert_eq!(matched, 3, "matched counts the full set, pre-limit");
    }

    #[test]
    fn arrange_filters_by_suite() {
        let entries = vec![
            entry("x", Some("alpha"), 100, RunSummary::default()),
            entry("y", Some("beta"), 200, RunSummary::default()),
            entry("z", Some("alpha"), 300, RunSummary::default()),
        ];
        let (out, matched) = arrange(entries, Some("alpha"), 0);
        assert_eq!(ids(&out), ["z", "x"]);
        assert_eq!(matched, 2);
    }

    #[test]
    fn latest_marker_only_on_pointer_target() {
        let entries = vec![
            entry("new", None, 300, RunSummary::default()),
            entry("old", None, 100, RunSummary::default()),
        ];
        let rows = to_local_rows(&entries, Some("new"));
        assert_eq!(rows[0].marker, '*');
        assert_eq!(rows[1].marker, ' ');

        let rows = to_local_rows(&entries, None);
        assert!(rows.iter().all(|r| r.marker == ' '));
    }

    #[test]
    fn severity_prefers_error_over_fail() {
        assert!(severity(0, 0).is_none());
        assert_eq!(severity(3, 0), Some(Sev::Fail));
        assert_eq!(severity(0, 2), Some(Sev::Error));
        assert_eq!(severity(3, 2), Some(Sev::Error));
    }

    #[test]
    fn git_format_variants() {
        assert_eq!(
            format_git_parts(Some("main"), Some("deadbeefcafe"), false),
            "main@deadbee"
        );
        assert_eq!(
            format_git_parts(Some("main"), Some("deadbeefcafe"), true),
            "main@deadbee+"
        );
        assert_eq!(format_git_parts(Some("main"), None, false), "main");
        assert_eq!(
            format_git_parts(None, Some("deadbeefcafe"), false),
            "deadbee"
        );
        assert_eq!(format_git_parts(None, None, true), "");
    }

    #[test]
    fn cost_only_when_positive() {
        assert_eq!(format_cost(Some(0.5)), "$0.5000");
        assert_eq!(format_cost(Some(0.0)), "");
        assert_eq!(format_cost(None), "");
    }

    #[test]
    fn render_table_plain_lists_ids_marks_latest_and_footers_without_escapes() {
        let entries = vec![
            entry("run-newer", Some("s"), 300, RunSummary::default()),
            entry("run-older", Some("s"), 100, RunSummary::default()),
        ];
        let rows = to_local_rows(&entries, Some("run-newer"));
        let text = render_table(&rows, &Palette::disabled(), "2 runs in .domarinn/runs");
        assert!(!text.contains('\x1b'), "disabled palette emits no escapes");
        assert!(text.contains("run-newer"));
        assert!(text.contains("run-older"));
        assert!(text.contains("* run-newer"), "latest carries the marker");
        assert!(text.contains("2 runs in .domarinn/runs"));
        // The header names every always-present column.
        assert!(text.contains("run id"));
        assert!(text.contains("pass"));
    }

    #[test]
    fn render_table_omits_cost_column_when_no_row_has_cost() {
        let entries = vec![entry("r", Some("s"), 100, RunSummary::default())];
        let rows = to_local_rows(&entries, None);
        let text = render_table(&rows, &Palette::disabled(), "1 run in .domarinn/runs");
        assert!(
            !text.contains("cost"),
            "no cost column when every cost is absent"
        );
    }

    #[test]
    fn render_table_shows_cost_column_when_present() {
        let summary = RunSummary {
            total: 1,
            passed: 1,
            cost_usd: Some(0.1234),
            ..Default::default()
        };
        let entries = vec![entry("r", Some("s"), 100, summary)];
        let rows = to_local_rows(&entries, None);
        let text = render_table(&rows, &Palette::disabled(), "1 run in .domarinn/runs");
        assert!(text.contains("cost"), "cost header present");
        assert!(text.contains("$0.1234"), "cost value present");
    }

    #[test]
    fn remote_response_deserializes_and_maps_to_rows() {
        let body = r#"{
            "runs": [
                {
                    "id": "r-1",
                    "project": "p",
                    "suite": "suite-a",
                    "created_at": "2026-01-01T00:00:00+00:00",
                    "git_branch": "main",
                    "git_commit": "abcdef1234567",
                    "git_dirty": true,
                    "case_count": 4,
                    "pass_count": 3,
                    "fail_count": 1,
                    "error_count": 0,
                    "pass_rate": 0.75,
                    "prompt_tokens": 10,
                    "completion_tokens": 20,
                    "cost_usd": 0.5,
                    "duration_ms": 65000,
                    "tags": []
                }
            ],
            "next_cursor": "abc"
        }"#;
        let resp: RemoteResponse = serde_json::from_str(body).unwrap();
        assert_eq!(resp.runs.len(), 1);
        assert_eq!(resp.next_cursor.as_deref(), Some("abc"));
        let rows = to_remote_rows(&resp.runs);
        let r = &rows[0];
        assert_eq!(r.id, "r-1");
        assert_eq!(r.suite, "suite-a");
        assert_eq!(r.score, "3/4");
        assert_eq!(r.sev, Some(Sev::Fail));
        assert_eq!(r.git, "main@abcdef1+");
        assert_eq!(r.cost, "$0.5000");
        assert_eq!(r.duration, "1m 05s");
    }
}
