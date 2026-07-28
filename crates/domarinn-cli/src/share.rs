//! The `domarinn share` command: upload a run to a server and print its URL.
//!
//! Best-effort by default (a failed upload warns and exits 0); `--strict` makes
//! upload failure fail the command. Git and CI metadata are attached
//! automatically when available.

use std::path::Path;

use clap::Args;
use domarinn_core::provenance;
use domarinn_core::result::RunResult;

use crate::exit;
use crate::loadrun::load_run;

#[derive(Args)]
pub struct ShareArgs {
    /// Run to upload: an id, `latest`, a result.json, or a run directory
    /// (default: the latest run).
    pub run: Option<String>,

    /// Fail the command (nonzero exit) if the upload fails.
    #[arg(long)]
    pub strict: bool,
}

pub fn execute(args: ShareArgs, server_url: Option<String>) -> u8 {
    let result = match load_run(args.run.as_deref().unwrap_or("latest")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    match upload_run(&result, server_url.as_deref(), args.strict) {
        Ok(_) => exit::OK,
        Err(e) => {
            if args.strict {
                eprintln!("error: {e}");
                exit::INFRA
            } else {
                eprintln!("warning: share failed: {e}");
                exit::OK
            }
        }
    }
}

/// Enrich and upload a run to the server, printing the resulting view URL.
///
/// Returns the view URL on success, so `run --share` can record it on the run
/// it is about to persist; returns an error if no server is configured or the
/// upload fails, and callers decide whether that is fatal (`--strict`) or
/// best-effort.
pub fn upload_run(
    result: &RunResult,
    server_url: Option<&str>,
    _strict: bool,
) -> Result<String, String> {
    let server = server_url
        .map(String::from)
        .or_else(|| std::env::var("DOMARINN_SERVER_URL").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "no server URL (set --server-url or DOMARINN_SERVER_URL)".to_string())?;

    let mut enriched = result.clone();
    enrich(&mut enriched);
    // `share_url` records where a run *landed*, not what it *is*, so it must never
    // travel with the document. Ingest is idempotent on
    // `sha256(canonical_json(run))` keyed by run id: re-uploading a run that had
    // recorded the URL of its own previous upload would hash differently and turn
    // a harmless re-share into a 409 Conflict. Stripping it keeps the uploaded
    // bytes identical every time.
    enriched.share_url = None;

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let url = runtime.block_on(upload(&server, &enriched))?;
    println!("View run: {url}");
    Ok(url)
}

/// Attach git and CI metadata to a run that predates engine-side collection.
///
/// Since the engine collects provenance itself, a freshly produced run already
/// carries both and this is a no-op. It stays as the backfill path for
/// `domarinn share <old-result.json>` — and for a run whose author set
/// `DOMARINN_PROVENANCE=off`, where the `is_none()` guards keep the opt-out
/// intact only because collection here would re-add what the author suppressed.
/// That is why it consults the same policy rather than collecting blindly.
fn enrich(result: &mut RunResult) {
    if provenance::ProvenanceOptions::from_env().mode == provenance::ProvenanceMode::Off {
        return;
    }
    if result.git.is_none() {
        result.git = provenance::collect_git(Path::new("."));
    }
    if result.ci.is_none() {
        result.ci = provenance::collect_ci();
    }
}

async fn upload(server: &str, result: &RunResult) -> Result<String, String> {
    let url = format!("{}/api/v1/runs", server.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let mut request = client.post(&url).json(result);
    if let Ok(token) = std::env::var("DOMARINN_TOKEN") {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {}: {body}", status.as_u16()));
    }
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    value
        .get("url")
        .and_then(|u| u.as_str())
        .map(String::from)
        .ok_or_else(|| "server response missing 'url'".to_string())
}
