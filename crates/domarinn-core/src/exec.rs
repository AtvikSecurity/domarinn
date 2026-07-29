//! One-shot exec protocol transport.
//!
//! Spawns a child, writes exactly one JSON request to its stdin, closes stdin,
//! and reads one JSON document from stdout with a timeout. Shared by exec
//! providers, exec asserts, and test generators. A nonzero exit or unparseable
//! stdout is an infrastructure error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A digest of the bytes behind a command — **reported, never keyed**.
///
/// # Why this is not in the cache key
///
/// It used to be. `program_identity` folded the contents of every argument that
/// named a readable file into the `exec` fingerprint, so that rebuilding the
/// binary busted its entries automatically.
///
/// The rule that replaced it is *hash what is sent, name what receives it*.
/// `command` and `env` **select** a provider, exactly as `model` and `base_url`
/// select an `anthropic` one; the rendered prompt, vars and tools **are** the
/// request. A cache key is built from those and from nothing else — in
/// particular from no property of the local filesystem, so it is identical on
/// every machine, in every checkout, from any working directory.
///
/// domarinn already applied that rule to the other half of the problem, and
/// [`crate::cache_key`] states it plainly: the model a provider *reports* using
/// is deliberately excluded from the key, because chasing it "would silently
/// discard every cache entry the day a vendor rolls a snapshot". A vendor
/// repointing `claude-opus-5` at new weights and an engineer rebuilding `./sut`
/// are the same event seen from two sides. Keying the local one and not the
/// remote one was an inconsistency, and an expensive one: no CI that builds its
/// provider from source could ever hit a shared cache, which is the entire
/// purpose of the S3 and results-server backends.
///
/// # What it is for instead
///
/// The property genuinely lost is that a rebuild is no longer *self-announcing*.
/// So this digest survives as what `CaseResult.model` is for the remote case —
/// evidence, carried on the entry, that makes drift visible and diffable. A hit
/// whose stored digest disagrees with the live one warns that the entry was
/// produced by a different build and points at `cache_salt`, the supported way
/// to say "this is a different version of the thing under test".
///
/// Because it never reaches a key, a differing digest costs nothing and busts
/// nothing. That is the whole point: the expensive answer is a *choice*, and it
/// belongs to the suite rather than to the filesystem.
///
/// # Shape
///
/// One blake3 over every argument that resolves to a readable file, folded in
/// argv order with its index, so moving an argument registers as a change.
/// `None` when nothing resolves — `docker run …`, a shell builtin, an inline
/// `sh -c` script — which means "no evidence either way", not "unchanged".
///
/// Resolution matches how the child will resolve it: relative arguments against
/// `base_dir` (the directory the child is spawned in, not the process cwd), and
/// `command[0]` additionally through `PATH` when it names no directory.
/// Arguments over [`MAX_HASHED_BYTES`] contribute their length instead of their
/// contents, bounding the cost of a pathological argument such as a
/// multi-gigabyte model file passed on the command line.
pub fn program_digest(command: &[String], base_dir: Option<&Path>) -> Option<String> {
    let mut hasher = blake3::Hasher::new();
    let mut found = false;
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
        found = true;
        // The index, so two arguments swapping places is a different program
        // even when the same bytes are present in both orders.
        hasher.update(&(i as u64).to_le_bytes());
        if meta.len() > MAX_HASHED_BYTES || hash_file_into(&mut hasher, &resolved).is_none() {
            // Too large to read, or unreadable despite stat'ing. Length alone is
            // a weak signal, but this is evidence rather than a key: a missed
            // rebuild here costs a warning that does not fire, not a wrong
            // cache hit. Note there is deliberately no `mtime` fallback — it
            // would report a fresh checkout as a different build on every
            // machine, which is noise dressed up as a diagnostic.
            hasher.update(&meta.len().to_le_bytes());
        }
    }
    found.then(|| format!("blake3:{}", hasher.finalize().to_hex()))
}

/// Files at or below this size contribute their contents to
/// [`program_digest`]; larger ones contribute their length. blake3 runs at
/// roughly a gigabyte per second and this is paid once per provider
/// construction, so the cap bounds the pathological argument rather than the
/// ordinary binary.
pub const MAX_HASHED_BYTES: u64 = 256 * 1024 * 1024;

/// Stream a file into `hasher`, or `None` when it could not be read.
fn hash_file_into(hasher: &mut blake3::Hasher, path: &Path) -> Option<()> {
    let mut file = std::fs::File::open(path).ok()?;
    std::io::copy(&mut file, hasher).ok()?;
    Some(())
}

/// Where on disk `arg` points, resolved the way the spawned child would resolve
/// it. `None` when the argument cannot name a file at all.
///
/// Visible to [`crate::cache_migrate`], which must resolve arguments exactly as
/// the historical key shapes did in order to find their entries.
pub(crate) fn resolve_program_arg(
    arg: &str,
    is_program: bool,
    base_dir: Option<&Path>,
) -> Option<PathBuf> {
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
