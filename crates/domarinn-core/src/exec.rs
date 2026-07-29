//! One-shot exec protocol transport.
//!
//! Spawns a child, writes exactly one JSON request to its stdin, closes stdin,
//! and reads one JSON document from stdout with a timeout. Shared by exec
//! providers, exec asserts, and test generators. A nonzero exit or unparseable
//! stdout is an infrastructure error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// An identity for the *program* a command runs, not just its argv.
///
/// This is what makes caching an `exec` provider or assertion safe by default.
/// `command` does not move when the binary behind it is rebuilt, so a key over
/// argv alone would serve stale output from every entry after a rebuild —
/// silently, and worst of all in CI. That hazard is the reason exec caching
/// used to be opt-in behind a hand-managed `cache_salt`.
///
/// Every argument that names a readable file contributes its path and a digest
/// of its **contents**. That covers the two shapes that matter — a compiled
/// binary (`./sut`) and an interpreter plus a script (`python3 grade.py`).
///
/// Contents rather than `mtime`, which is what this used to key on. `mtime` is
/// machine-local: `git` does not record it, so a fresh checkout stamps every
/// file with its checkout time and no two runners ever agree. That made the
/// fingerprint unshareable across machines, which silently disabled the S3 and
/// results-server caches for every `exec` provider — the opposite of what those
/// backends exist for. A content digest is identical wherever the same bytes
/// land, so a warm shared cache survives a fresh clone.
///
/// The cost is one read per argument, paid once per provider construction
/// rather than per cache lookup, so the ordinary case is a few milliseconds.
/// Arguments larger than [`MAX_HASHED_BYTES`] fall back to length and mtime —
/// a bounded escape for a pathological argument (a multi-gigabyte model file
/// passed on the command line), not the common path.
///
/// # Resolved the way the child will resolve it
///
/// `base_dir` is the directory the child is spawned in, and a relative argument
/// is resolved against *it*, not the process cwd. Getting this wrong is not a
/// near-miss: `command: ["./sut"]` in a suite run from a repo root stats a path
/// that does not exist, contributes nothing, and silently degrades the key back
/// to argv alone — the exact failure this function exists to prevent.
///
/// `command[0]` additionally resolves through `PATH` when it names no directory,
/// because `my-agent` installed on `PATH` is the most common exec provider there
/// is and it is precisely the one a rebuild moves. Later arguments never do: a
/// bare `grade.py` is a file next to the suite, not a program.
///
/// The recorded `path` is always the *argv* string, never the resolved one, so
/// two machines sharing a cache do not miss on each other purely for having
/// checked the repository out somewhere else.
///
/// Arguments that name nothing readable (flags, plain values, an inline `sh -c`
/// script) contribute nothing beyond already being in `command`. When *no*
/// argument resolves, there is no program identity at all and the caller must
/// treat the command as uncacheable without an explicit `cache_salt` — see
/// [`crate::exec_provider::ExecProvider::cacheable`].
pub fn program_identity(command: &[String], base_dir: Option<&Path>) -> serde_json::Value {
    let mut parts = Vec::new();
    for (i, arg) in command.iter().enumerate() {
        let Some(resolved) = resolve_program_arg(arg, i == 0, base_dir) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&resolved) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        parts.push(match content_digest(&resolved, meta.len()) {
            Some(content) => serde_json::json!({ "path": arg, "content": content }),
            // Too large to hash, or unreadable despite stat'ing. Fall back to
            // the metadata this function used to key on exclusively. The shape
            // differs from the hashed one, so the two can never collide.
            None => {
                // Nanoseconds as well as seconds: a rebuild that lands in the
                // same second and happens to produce an equal-length artifact
                // is ordinary, and second-granularity would not see it.
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                serde_json::json!({
                    "path": arg,
                    "len": meta.len(),
                    "mtime": mtime.map(|d| d.as_secs()),
                    "mtime_ns": mtime.map(|d| d.subsec_nanos()),
                })
            }
        });
    }
    serde_json::Value::Array(parts)
}

/// Files at or below this size are keyed on their contents; larger ones fall
/// back to stat metadata. blake3 runs at roughly a gigabyte per second and this
/// is paid once per provider construction, so the cap exists to bound the
/// pathological argument rather than the ordinary binary.
pub const MAX_HASHED_BYTES: u64 = 256 * 1024 * 1024;

/// blake3 of a file's contents, or `None` when it is too large to hash or could
/// not be read — in which case the caller falls back to stat metadata.
fn content_digest(path: &Path, len: u64) -> Option<String> {
    if len > MAX_HASHED_BYTES {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("blake3:{}", hasher.finalize().to_hex()))
}

/// Where on disk `arg` points, resolved the way the spawned child would resolve
/// it. `None` when the argument cannot name a file at all.
fn resolve_program_arg(arg: &str, is_program: bool, base_dir: Option<&Path>) -> Option<PathBuf> {
    if arg.is_empty() {
        return None;
    }
    let path = Path::new(arg);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    // A bare name naming no directory is not a relative path *for the program*
    // — the OS resolves that one through `PATH`. Every later argument is an
    // ordinary path relative to the child's cwd.
    if is_program && path.components().count() == 1 {
        return path_lookup(arg);
    }
    Some(match base_dir {
        Some(dir) => dir.join(path),
        None => path.to_path_buf(),
    })
}

/// The first entry on `PATH` that holds a file named `name`.
fn path_lookup(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Whether [`program_identity`] found anything at all to key on.
pub fn has_program_identity(identity: &serde_json::Value) -> bool {
    identity.as_array().is_some_and(|parts| !parts.is_empty())
}

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
        match stdin.write_all(&request_bytes).await {
            Ok(()) => {}
            // A child that does not read its stdin (and may have already exited)
            // closes the pipe; that is legitimate, so proceed to read its output
            // rather than failing the whole call.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(source) => {
                return Err(ExecError::Io {
                    command: command.to_vec(),
                    source,
                })
            }
        }
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
