//! The `domarinn baseline` subcommand: manage the server-side pin from a
//! terminal.
//!
//! `show`/`set`/`clear` address the suite named by the local `domarinn.yaml`
//! (the same `project:`/`suite:` pair `--against server:baseline` resolves
//! through) and talk to the same endpoints the web UI's pin button uses. The
//! typical bootstrap is one command: `domarinn baseline set --branch main`.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde_json::json;

use crate::exit;

#[derive(Args)]
pub struct BaselineArgs {
    #[command(subcommand)]
    cmd: BaselineCmd,
}

#[derive(Subcommand)]
enum BaselineCmd {
    /// Show the suite's pinned baseline.
    Show {
        /// Suite file or directory naming the project/suite.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Pin a run (by reference) or a branch (`--branch`) as the baseline.
    Set {
        /// Run to pin: `latest`, a run id, or a path to a result.json.
        run: Option<String>,
        /// Pin a branch instead: the newest runs on it merge into the
        /// comparison baseline, advancing automatically as new runs land.
        #[arg(long, conflicts_with = "run")]
        branch: Option<String>,
        /// Suite file or directory naming the project/suite.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Remove the pin. Doing nothing when nothing is pinned is success.
    Clear {
        /// Suite file or directory naming the project/suite.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

pub fn execute(args: BaselineArgs, server_url: Option<String>) -> u8 {
    match args.cmd {
        BaselineCmd::Show { path } => show(&path, server_url),
        BaselineCmd::Set { run, branch, path } => set(run, branch, &path, server_url),
        BaselineCmd::Clear { path } => clear(&path, server_url),
    }
}

/// The addressing every subcommand needs: which server, and which
/// `(project, suite)` on it.
struct Target {
    server: String,
    project: String,
    suite: String,
}

/// Resolve the target or print the usage error and return its exit code.
fn target(path: &Path, server_url: Option<String>) -> Result<Target, u8> {
    let Some(server) = server_url
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("DOMARINN_SERVER_URL").ok())
        .filter(|s| !s.is_empty())
    else {
        eprintln!(
            "error: `domarinn baseline` needs a server (set --server-url or DOMARINN_SERVER_URL)"
        );
        return Err(exit::USAGE);
    };
    let suite = match domarinn_core::loader::load_file(path) {
        Ok(suite) => suite,
        Err(e) => {
            eprintln!("error: {e}");
            return Err(exit::USAGE);
        }
    };
    let (Some(project), Some(suite)) = (suite.project, suite.suite) else {
        eprintln!(
            "error: `domarinn baseline` needs both `project:` and `suite:` set in the suite \
             config; the server keys baselines on (project, suite)"
        );
        return Err(exit::USAGE);
    };
    Ok(Target {
        server,
        project,
        suite,
    })
}

fn show(path: &Path, server_url: Option<String>) -> u8 {
    let target = match target(path, server_url) {
        Ok(t) => t,
        Err(code) => return code,
    };
    let (status, body) = match request(&target, reqwest::Method::GET, None) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    let parsed: Option<serde_json::Value> = serde_json::from_str(&body).ok();
    if status == reqwest::StatusCode::NOT_FOUND && code_of(&parsed) == Some("baseline_unpinned") {
        println!(
            "no baseline pinned for {}/{}. Pin one with `domarinn baseline set --branch <name>` \
             or `domarinn baseline set <run>`.",
            target.project, target.suite
        );
        return exit::OK;
    }
    if !status.is_success() {
        eprintln!("error: GET baseline: HTTP {}: {body}", status.as_u16());
        return exit::INFRA;
    }
    let field = |key: &str| {
        parsed
            .as_ref()
            .and_then(|v| v.get(key)?.as_str())
            .map(str::to_string)
    };
    match (field("branch"), field("run_id")) {
        (Some(branch), _) => println!(
            "baseline for {}/{}: branch {branch} (auto-tracking: the newest runs on the branch \
             merge into the comparison)",
            target.project, target.suite
        ),
        (None, Some(run_id)) => println!(
            "baseline for {}/{}: run {run_id} (fixed)",
            target.project, target.suite
        ),
        (None, None) => {
            eprintln!("error: unrecognized baseline response: {body}");
            return exit::INFRA;
        }
    }
    exit::OK
}

fn set(run: Option<String>, branch: Option<String>, path: &Path, server_url: Option<String>) -> u8 {
    let target = match target(path, server_url) {
        Ok(t) => t,
        Err(code) => return code,
    };
    let body = match (run, branch) {
        (None, Some(branch)) => json!({ "branch": branch }),
        (Some(reference), None) => {
            // A local reference (`latest`, a path, a stored id) resolves to its
            // concrete id here — the server has no notion of "my latest". A
            // reference that is not resolvable locally but has the shape of an
            // id passes through verbatim: pinning a server-only run id from a
            // machine that never ran the suite is legitimate.
            let run_id = match crate::loadrun::load_run(&reference) {
                Ok(run) => run.run_id.as_str().to_string(),
                Err(_) if !reference.contains(['/', '\\']) && reference != "latest" => reference,
                Err(e) => {
                    eprintln!("error: {e}");
                    return exit::USAGE;
                }
            };
            json!({ "run_id": run_id })
        }
        (None, None) => {
            eprintln!("error: `domarinn baseline set` needs a run reference or --branch <name>");
            return exit::USAGE;
        }
        // Unreachable behind clap's conflicts_with, kept total.
        (Some(_), Some(_)) => {
            eprintln!("error: pass a run reference or --branch, not both");
            return exit::USAGE;
        }
    };
    let (status, reply) = match request(&target, reqwest::Method::PUT, Some(body.clone())) {
        Ok(r) => r,
        Err(code) => return code,
    };
    if status == reqwest::StatusCode::NOT_FOUND {
        eprintln!(
            "error: the server does not know that run for {}/{} — upload it first (`domarinn \
             run --share`), or check the id: {reply}",
            target.project, target.suite
        );
        return exit::USAGE;
    }
    if !status.is_success() {
        eprintln!("error: PUT baseline: HTTP {}: {reply}", status.as_u16());
        return exit::INFRA;
    }
    match body.get("branch").and_then(|b| b.as_str()) {
        Some(branch) => println!(
            "pinned branch {branch} as the baseline for {}/{}",
            target.project, target.suite
        ),
        None => println!(
            "pinned run {} as the baseline for {}/{}",
            body["run_id"].as_str().unwrap_or("?"),
            target.project,
            target.suite
        ),
    }
    exit::OK
}

fn clear(path: &Path, server_url: Option<String>) -> u8 {
    let target = match target(path, server_url) {
        Ok(t) => t,
        Err(code) => return code,
    };
    let (status, body) = match request(&target, reqwest::Method::DELETE, None) {
        Ok(r) => r,
        Err(code) => return code,
    };
    if status == reqwest::StatusCode::NOT_FOUND {
        // Idempotent: the requested end state holds.
        println!("nothing was pinned for {}/{}", target.project, target.suite);
        return exit::OK;
    }
    if !status.is_success() {
        eprintln!("error: DELETE baseline: HTTP {}: {body}", status.as_u16());
        return exit::INFRA;
    }
    println!(
        "cleared the baseline for {}/{}",
        target.project, target.suite
    );
    exit::OK
}

/// The `code` field of a JSON error body, when there is one.
fn code_of(parsed: &Option<serde_json::Value>) -> Option<&str> {
    parsed.as_ref()?.get("code")?.as_str()
}

/// One request to the suite's baseline endpoint. Network-level failure is
/// infrastructure (exit 3), printed here so every caller reports it the same
/// way.
fn request(
    target: &Target,
    method: reqwest::Method,
    body: Option<serde_json::Value>,
) -> Result<(reqwest::StatusCode, String), u8> {
    let url = format!(
        "{}/api/v1/projects/{}/suites/{}/baseline",
        target.server.trim_end_matches('/'),
        urlencoding(&target.project),
        urlencoding(&target.suite),
    );
    let runtime = tokio::runtime::Runtime::new().map_err(|e| {
        eprintln!("error: starting runtime: {e}");
        exit::INFRA
    })?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                eprintln!("error: {e}");
                exit::INFRA
            })?;
        let mut request = client.request(method.clone(), &url);
        if let Ok(token) = std::env::var("DOMARINN_TOKEN") {
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|e| {
            eprintln!("error: {method} {url}: {e}");
            exit::INFRA
        })?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        Ok((status, text))
    })
}

/// Percent-encode one path segment, so a project or suite containing `/` or a
/// space cannot break out of its position in the path.
fn urlencoding(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
