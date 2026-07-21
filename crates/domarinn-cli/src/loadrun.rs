//! Loading a stored [`RunResult`] by run id, file path, or `latest`.

use std::path::{Path, PathBuf};

use domarinn_core::RunResult;

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

fn runs_dir() -> PathBuf {
    Path::new(".domarinn").join("runs")
}
