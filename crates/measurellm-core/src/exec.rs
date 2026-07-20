//! One-shot exec protocol transport.
//!
//! Spawns a child, writes exactly one JSON request to its stdin, closes stdin,
//! and reads one JSON document from stdout with a timeout. Shared by exec
//! providers, exec asserts, and test generators. A nonzero exit or unparseable
//! stdout is an infrastructure error.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value as Json;
use tokio::io::AsyncWriteExt;

use crate::exec_protocol::{PROTOCOL_ENV, PROTOCOL_VERSION};

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("spawning {command:?}: {source}")]
    Spawn {
        command: Vec<String>,
        #[source]
        source: std::io::Error,
    },
    #[error("{command:?} timed out after {ms} ms")]
    Timeout { command: Vec<String>, ms: u64 },
    #[error("{command:?} exited with {code:?}: {stderr}")]
    NonZero {
        command: Vec<String>,
        code: Option<i32>,
        stderr: String,
    },
    #[error("{command:?} produced invalid JSON on stdout: {message}")]
    BadJson {
        command: Vec<String>,
        message: String,
    },
    #[error("i/o error running {command:?}: {source}")]
    Io {
        command: Vec<String>,
        #[source]
        source: std::io::Error,
    },
}

impl ExecError {
    /// Whether retrying could plausibly help.
    pub fn is_retriable(&self) -> bool {
        matches!(self, ExecError::Timeout { .. } | ExecError::Spawn { .. })
    }
}

/// Run a command under the exec protocol, sending `request` and returning the
/// parsed JSON response. Returns the raw stdout string alongside for JSONL
/// callers via [`run_exec_raw`]; this helper parses a single JSON document.
pub async fn run_exec_json(
    command: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&Path>,
    timeout: Duration,
    request: &Json,
) -> Result<Json, ExecError> {
    let stdout = run_exec_raw(command, env, cwd, timeout, request).await?;
    serde_json::from_str(stdout.trim()).map_err(|e| ExecError::BadJson {
        command: command.to_vec(),
        message: e.to_string(),
    })
}

/// Like [`run_exec_json`] but returns the raw stdout string (for JSONL parsing).
pub async fn run_exec_raw(
    command: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&Path>,
    timeout: Duration,
    request: &Json,
) -> Result<String, ExecError> {
    if command.is_empty() {
        return Err(ExecError::Spawn {
            command: command.to_vec(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command"),
        });
    }

    let mut cmd = tokio::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    cmd.env(PROTOCOL_ENV, PROTOCOL_VERSION.to_string());
    for (k, v) in env {
        cmd.env(k, v);
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|source| ExecError::Spawn {
        command: command.to_vec(),
        source,
    })?;

    let request_bytes = serde_json::to_vec(request).map_err(|e| ExecError::BadJson {
        command: command.to_vec(),
        message: format!("serializing request: {e}"),
    })?;

    // Write the request, then let the child produce its response.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&request_bytes)
            .await
            .map_err(|source| ExecError::Io {
                command: command.to_vec(),
                source,
            })?;
        // Drop closes stdin, signaling EOF to the child.
        drop(stdin);
    }

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => result.map_err(|source| ExecError::Io {
            command: command.to_vec(),
            source,
        })?,
        Err(_) => {
            return Err(ExecError::Timeout {
                command: command.to_vec(),
                ms: timeout.as_millis() as u64,
            })
        }
    };

    if !output.status.success() {
        return Err(ExecError::NonZero {
            command: command.to_vec(),
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn timeout() -> Duration {
        Duration::from_secs(10)
    }

    #[tokio::test]
    async fn runs_a_command_and_parses_json() {
        // `cat` echoes our request back; the request is valid JSON, so it parses.
        let req = json!({"hello": "world"});
        let out = run_exec_json(
            &["cat".to_string()],
            &BTreeMap::new(),
            None,
            timeout(),
            &req,
        )
        .await
        .unwrap();
        assert_eq!(out, req);
    }

    #[tokio::test]
    async fn nonzero_exit_is_an_error() {
        let err = run_exec_json(
            &["false".to_string()],
            &BTreeMap::new(),
            None,
            timeout(),
            &json!({}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ExecError::NonZero { .. }));
    }

    #[tokio::test]
    async fn bad_json_is_an_error() {
        let err = run_exec_json(
            &["printf".to_string(), "not json".to_string()],
            &BTreeMap::new(),
            None,
            timeout(),
            &json!({}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ExecError::BadJson { .. }));
    }

    #[tokio::test]
    async fn missing_command_is_a_spawn_error() {
        let err = run_exec_json(
            &["definitely-not-a-real-binary-xyz".to_string()],
            &BTreeMap::new(),
            None,
            timeout(),
            &json!({}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ExecError::Spawn { .. }));
        assert!(err.is_retriable());
    }
}
