//! The `domarinn share` command: upload a run to a server and print its URL.
//!
//! Best-effort by default (a failed upload warns and exits 0); `--strict` makes
//! upload failure fail the command. Git and CI metadata are attached
//! automatically when available.
//!
//! `run --share` is the other way round — strict by default, with
//! `--allow-share-failure` to opt out. The asymmetry is deliberate: `share` is
//! run by hand against a run that already exists on disk, so a failed upload
//! costs a retry, while `run --share` is a CI step whose whole purpose is to
//! store the results, and exiting 0 having stored nothing reports a green job
//! for work that went nowhere.

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
    match upload_run(&result, server_url.as_deref()) {
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
/// upload fails. Whether that is fatal is the caller's policy, not this
/// function's — `share` is best-effort unless `--strict`, `run --share` is fatal
/// unless `--allow-share-failure` — so it takes no flag of its own.
pub fn upload_run(result: &RunResult, server_url: Option<&str>) -> Result<String, String> {
    let server = resolve_server(server_url)
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

/// Ask the server what it accepts *before* the run spends anything.
///
/// `run --share` failing closed still only reports the skew after every provider
/// call has been billed and the results have nowhere to go. This is the cheap
/// half of the same guarantee: one GET, before the runner starts, so a version
/// skew costs nothing but the round trip.
///
/// Deliberately lenient — only a server that answered, parsed, and named a
/// window this CLI is outside of earns a refusal. Unreachable, slow, 404 and
/// unparsable all proceed with a warning, because the POST is the authoritative
/// answer and a preflight that inferred a refusal from silence would turn every
/// network blip, proxy and older server into a run that never happened. An
/// absent or empty window is the same case: it states nothing to be outside of.
pub fn preflight_schema(server_url: Option<&str>) -> Result<(), String> {
    // No server is the share step's error to report later, with its own remedy;
    // refusing here would only print the same misconfiguration twice.
    let Some(server) = resolve_server(server_url) else {
        return Ok(());
    };

    let meta = match tokio::runtime::Runtime::new().map_err(|e| e.to_string()) {
        Ok(runtime) => runtime.block_on(fetch_meta(&server)),
        Err(e) => Err(e),
    };
    let meta = match meta {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                error = %e,
                server = %server,
                "could not check the server's schema window before running; \
                 continuing (the upload itself is authoritative)"
            );
            return Ok(());
        }
    };

    let Some(supported) = schema_window(&meta) else {
        // Same family as unreachable and unparsable, and warned about the same
        // way: the server answered, but not with a window this run can be
        // outside of. Silence here would be the one case where a preflight that
        // checked nothing is indistinguishable from one that passed.
        tracing::warn!(
            server = %server,
            "the server did not state a usable schema window, so nothing could \
             be checked before running; continuing (the upload itself is \
             authoritative)"
        );
        return Ok(());
    };
    if supported.contains(&domarinn_core::RESULT_SCHEMA_VERSION) {
        return Ok(());
    }

    let server_version = meta
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    Err(mismatch_message(&server, &supported, server_version))
}

/// The versions a `/meta` body says it accepts, or `None` if it did not state a
/// window this CLI can reason about.
///
/// All-or-nothing on purpose. Skipping the entries that fail to parse would
/// narrow `[0, "2"]` to `[0]` and then *confidently refuse* a run over a window
/// the server never advertised — a false refusal, quoting numbers back at the
/// operator that they will not find anywhere in their server's response. This
/// function's entire licence to fail a run is that the mismatch is confirmed, so
/// anything it cannot read in full it does not read at all.
fn schema_window(meta: &serde_json::Value) -> Option<Vec<u32>> {
    let window: Vec<u32> = meta
        .get("supported_schema_versions")?
        .as_array()?
        .iter()
        .map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
        .collect::<Option<_>>()?;
    // An empty window states nothing to be outside of.
    (!window.is_empty()).then_some(window)
}

/// The refusal an operator has to act on.
///
/// Split out to be testable without a server: which side to upgrade is the whole
/// actionable content of the message and is not guessable from the numbers alone
/// by someone reading a failed CI job in a hurry — a window entirely below ours
/// is a server too old to store what we write, anything else is a CLI too old to
/// write what the server now stores. Getting that backwards sends them to
/// upgrade the one thing that was already current.
fn mismatch_message(server: &str, supported: &[u32], server_version: &str) -> String {
    let ours = domarinn_core::RESULT_SCHEMA_VERSION;
    let older = if supported.iter().all(|v| *v < ours) {
        "server"
    } else {
        "CLI"
    };
    format!(
        "server at {server} accepts result schema versions {supported:?} (server \
         v{server_version}); this CLI writes v{ours}. Upgrade the {older} before \
         sharing, or pass --allow-share-failure to run anyway."
    )
}

/// `GET /api/v1/meta`, parsed as loose JSON.
///
/// Loose on purpose: the CLI reads three fields out of a response the server is
/// free to grow, and importing its DTO would make every field the server adds a
/// compile-time coupling — and, worse, make an unknown field from a *newer*
/// server a parse failure, which is precisely the skew this preflight exists to
/// report clearly.
///
/// The timeout is short because this runs ahead of the work: a server that
/// cannot answer in five seconds has said nothing worth delaying a run for.
async fn fetch_meta(server: &str) -> Result<serde_json::Value, String> {
    let url = format!("{}/api/v1/meta", server.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let response = with_token(client.get(&url))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// `--server-url`, else `DOMARINN_SERVER_URL`, else nothing.
///
/// Shared so the preflight and the upload can never disagree about which server
/// they are talking to — a preflight that cleared a different host than the one
/// the POST went to would be worse than none.
fn resolve_server(server_url: Option<&str>) -> Option<String> {
    server_url
        .map(String::from)
        .or_else(|| std::env::var("DOMARINN_SERVER_URL").ok())
        .filter(|s| !s.is_empty())
}

/// Attach `DOMARINN_TOKEN` as a bearer when one is set.
fn with_token(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("DOMARINN_TOKEN") {
        Ok(token) if !token.is_empty() => request.bearer_auth(token),
        _ => request,
    }
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
    let response = with_token(client.post(&url).json(result))
        .send()
        .await
        .map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use domarinn_core::RESULT_SCHEMA_VERSION as OURS;

    /// A window entirely below ours is a server too old to store what this CLI
    /// writes, so the operator has to upgrade the *server*. Only the other arm
    /// is reachable from the end-to-end tests, and a direction this message gets
    /// backwards costs an upgrade of the one component that was already current.
    #[test]
    fn a_window_below_ours_says_to_upgrade_the_server() {
        // 0 is below every schema version domarinn has ever written, so this
        // stays a below-window case across bumps without naming a version.
        let msg = mismatch_message("http://s", &[0], "0.1.0");
        assert!(msg.contains("Upgrade the server"), "{msg}");
    }

    /// The mirror image: a window entirely above ours is a CLI too old to write
    /// what the server now stores.
    #[test]
    fn a_window_above_ours_says_to_upgrade_the_cli() {
        let msg = mismatch_message("http://s", &[OURS + 1], "9.9.9");
        assert!(msg.contains("Upgrade the CLI"), "{msg}");
    }

    /// A window straddling ours — the server accepts both an older and a newer
    /// version, just not this one — is the CLI's problem: there is a version it
    /// could write that the server would take.
    #[test]
    fn a_window_straddling_ours_says_to_upgrade_the_cli() {
        let msg = mismatch_message("http://s", &[0, OURS + 1], "9.9.9");
        assert!(msg.contains("Upgrade the CLI"), "{msg}");
    }

    /// A window with an entry this CLI cannot read is no window at all. Dropping
    /// the unreadable entry would leave `[0]`, which does not contain ours — and
    /// the caller would refuse the run over a window the server never sent.
    #[test]
    fn a_window_with_an_unreadable_entry_is_not_a_window() {
        let meta = serde_json::json!({
            "supported_schema_versions": [0, OURS.to_string()],
        });
        assert_eq!(schema_window(&meta), None);
    }

    /// An empty or absent window states nothing to be outside of.
    #[test]
    fn an_empty_or_absent_window_is_not_a_window() {
        assert_eq!(
            schema_window(&serde_json::json!({"supported_schema_versions": []})),
            None
        );
        assert_eq!(schema_window(&serde_json::json!({"name": "stub"})), None);
    }

    /// The happy path, so the `None`s above are not passing for want of any
    /// window ever parsing.
    #[test]
    fn a_well_formed_window_parses() {
        let meta = serde_json::json!({"supported_schema_versions": [1, OURS]});
        assert_eq!(schema_window(&meta), Some(vec![1, OURS]));
    }
}
