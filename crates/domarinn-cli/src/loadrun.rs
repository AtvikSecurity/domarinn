//! Loading a stored [`RunResult`] by run id, file path, or `latest`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use domarinn_core::RunResult;
use serde::Deserialize;

/// Resolve a run reference to a `result.json` path.
///
/// Accepts `latest`, a path to a `result.json` or a run directory, or a bare run
/// id under `.domarinn/runs/`.
pub fn resolve_run_path(reference: &str) -> Result<PathBuf, String> {
    if reference == "latest" {
        let latest = runs_dir().join("latest");
        let id = std::fs::read_to_string(&latest)
            .map_err(|_| "no latest run found; run a suite first".to_string())?;
        return Ok(runs_dir().join(id.trim()).join("result.json"));
    }
    let path = Path::new(reference);
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if path.is_dir() {
        return Ok(path.join("result.json"));
    }
    // Treat as a run id.
    let candidate = runs_dir().join(reference).join("result.json");
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(format!("could not resolve run reference '{reference}'"))
}

/// Load a run by reference.
pub fn load_run(reference: &str) -> Result<RunResult, String> {
    let path = resolve_run_path(reference)?;
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))
}

/// The local run store: `.domarinn/runs`, holding one `<run_id>/result.json`
/// per persisted run plus a plain-text `latest` pointer file.
pub(crate) fn runs_dir() -> PathBuf {
    Path::new(".domarinn").join("runs")
}

/// Just enough of a stored run to decide whether it is the same suite as
/// another, and which of two is newer.
#[derive(Deserialize)]
struct RunIdent {
    run_id: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    suite: Option<String>,
    finished_at: DateTime<Utc>,
}

/// The newest stored run of the same `(project, suite)` as `head`, excluding
/// `head` itself.
///
/// `--against latest` must not use the `latest` pointer file. That pointer
/// records the last run of *any* suite — `output::persist` rewrites it on every
/// run — so in a repo with more than one suite it silently diffs one suite
/// against another, and `diff_runs` joins on `case_key` without a suite guard,
/// so the result looks plausible rather than empty.
///
/// Scanning is affordable here for the same reason it is in `domarinn runs`:
/// the store is a developer's local history, and a baseline lookup happens once
/// per run.
pub fn latest_for_suite(head: &RunResult) -> Option<PathBuf> {
    let dir = runs_dir();
    let mut best: Option<(DateTime<Utc>, PathBuf)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path().join("result.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // A corrupt or half-written run is skipped rather than fatal: it must
        // not be able to stop a run from finding an older, valid baseline.
        let Ok(run) = serde_json::from_str::<RunIdent>(&text) else {
            continue;
        };
        if run.run_id == head.run_id.as_str()
            || run.project != head.project
            || run.suite != head.suite
        {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(best, _)| run.finished_at > *best)
        {
            best = Some((run.finished_at, path));
        }
    }
    best.map(|(_, path)| path)
}
