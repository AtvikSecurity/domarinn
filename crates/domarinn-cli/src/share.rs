//! The `domarinn share` command: upload a run to a server and print its URL.
//!
//! Best-effort by default (a failed upload warns and exits 0); `--strict` makes
//! upload failure fail the command. Git and CI metadata are attached
//! automatically when available.

use std::process::Command;

use clap::Args;
use domarinn_core::result::{CiMeta, GitMeta, RunResult};

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
        Ok(()) => exit::OK,
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
/// Returns an error if no server is configured or the upload fails; callers
/// decide whether that is fatal (`--strict`) or best-effort.
pub fn upload_run(
    result: &RunResult,
    server_url: Option<&str>,
    _strict: bool,
) -> Result<(), String> {
    let server = server_url
        .map(String::from)
        .or_else(|| std::env::var("DOMARINN_SERVER_URL").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "no server URL (set --server-url or DOMARINN_SERVER_URL)".to_string())?;

    let mut enriched = result.clone();
    enrich(&mut enriched);

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let url = runtime.block_on(upload(&server, &enriched))?;
    println!("View run: {url}");
    Ok(())
}

/// Attach git and CI metadata to the run if not already present.
fn enrich(result: &mut RunResult) {
    if result.git.is_none() {
        result.git = collect_git();
    }
    if result.ci.is_none() {
        result.ci = collect_ci();
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn collect_git() -> Option<GitMeta> {
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]);
    let commit = git(&["rev-parse", "HEAD"]);
    if branch.is_none() && commit.is_none() {
        return None;
    }
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    Some(GitMeta {
        branch,
        commit,
        dirty,
    })
}

fn collect_ci() -> Option<CiMeta> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if env("GITHUB_ACTIONS").is_some() {
        let run_url = match (
            env("GITHUB_SERVER_URL"),
            env("GITHUB_REPOSITORY"),
            env("GITHUB_RUN_ID"),
        ) {
            (Some(server), Some(repo), Some(id)) => {
                Some(format!("{server}/{repo}/actions/runs/{id}"))
            }
            _ => None,
        };
        return Some(CiMeta {
            provider: Some("github".into()),
            run_url,
        });
    }
    if env("GITLAB_CI").is_some() {
        return Some(CiMeta {
            provider: Some("gitlab".into()),
            run_url: env("CI_JOB_URL"),
        });
    }
    if env("JENKINS_URL").is_some() {
        return Some(CiMeta {
            provider: Some("jenkins".into()),
            run_url: env("BUILD_URL"),
        });
    }
    if env("CI").is_some() {
        return Some(CiMeta {
            provider: Some("ci".into()),
            run_url: None,
        });
    }
    None
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
