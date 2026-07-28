//! Collecting who and what produced a run.
//!
//! This lives in the engine rather than the upload path on purpose. Git and CI
//! metadata used to be attached only by `domarinn share`, so a plain
//! `domarinn run` wrote `git: null` into `result.json` — `domarinn runs` showed
//! an empty git column for every unshared run, and a local `--against`
//! comparison could not attribute a regression to a commit.
//!
//! ## Privacy
//!
//! `actor` and `host` are mild PII (hostnames in particular tend to encode
//! people's names), and once written they are inside the document the server
//! content-hashes for ingest idempotency — so they cannot be redacted
//! afterwards without changing that hash. Suppression therefore has to happen
//! here, on the client, which is the only party that ever sees the values. See
//! [`ProvenanceMode`].

use std::path::Path;
use std::process::Command;

use crate::result::{CiMeta, GitMeta, RunOrigin};

/// The environment variable that sets the policy for a whole machine or image.
pub const ENV_MODE: &str = "DOMARINN_PROVENANCE";

/// How much provenance to record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProvenanceMode {
    /// Everything: actor, host, version, note, git, CI.
    #[default]
    Full,
    /// Drop `actor` and `host`; keep version, note, git and CI. Records
    /// `redacted: true` so a reader can tell suppression from an old client.
    Anonymous,
    /// Record nothing at all — `origin`, `git` and `ci` all stay `None`.
    Off,
}

impl ProvenanceMode {
    /// Parse a [`ENV_MODE`] value. `None` for anything unrecognized; callers
    /// warn and fall back to the default rather than failing, because refusing
    /// to evaluate anything over a misspelled metadata setting is a bad trade.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" | "on" | "true" | "1" => Some(ProvenanceMode::Full),
            "anonymous" | "anon" => Some(ProvenanceMode::Anonymous),
            "off" | "none" | "false" | "0" => Some(ProvenanceMode::Off),
            _ => None,
        }
    }

    fn records_identity(self) -> bool {
        matches!(self, ProvenanceMode::Full)
    }
}

/// What to record about a run's origin.
///
/// A run option rather than a suite-config field, for the same reason
/// `RunOptions::retries` is: `Suite` is serialized wholesale into
/// `config_snapshot` and hashed into `config_digest`, so a `provenance:` key in
/// the config would show up as spurious config drift in every comparison.
#[derive(Debug, Clone, Default)]
pub struct ProvenanceOptions {
    pub mode: ProvenanceMode,
    /// A human label for this run, from `--note`, falling back to the suite's
    /// `description` when unset.
    pub note: Option<String>,
}

impl ProvenanceOptions {
    /// Read the mode from [`ENV_MODE`], warning on an unrecognized value.
    pub fn from_env() -> Self {
        let mode = match std::env::var(ENV_MODE) {
            Ok(raw) if !raw.trim().is_empty() => match ProvenanceMode::parse(&raw) {
                Some(mode) => mode,
                None => {
                    tracing::warn!(
                        value = %raw,
                        "unrecognized {ENV_MODE}; expected full|anonymous|off, recording full"
                    );
                    ProvenanceMode::default()
                }
            },
            _ => ProvenanceMode::default(),
        };
        ProvenanceOptions { mode, note: None }
    }
}

/// Everything the engine records about a run's origin.
#[derive(Debug, Clone, Default)]
pub struct Collected {
    pub git: Option<GitMeta>,
    pub ci: Option<CiMeta>,
    pub origin: Option<RunOrigin>,
}

/// Collect provenance for a run of the suite rooted at `base_dir`.
pub fn collect(opts: &ProvenanceOptions, base_dir: &Path) -> Collected {
    if opts.mode == ProvenanceMode::Off {
        return Collected::default();
    }

    let identity = opts.mode.records_identity();
    let origin = RunOrigin {
        actor: identity.then(actor).flatten(),
        host: identity.then(host).flatten(),
        version: Some(crate::VERSION.to_string()),
        note: opts.note.clone(),
        // Only meaningful as a positive assertion; `false` on every full run
        // would be noise in every stored document.
        redacted: (!identity).then_some(true),
    };

    Collected {
        git: collect_git(base_dir),
        ci: collect_ci(),
        origin: Some(origin),
    }
}

/// Who to attribute the run to.
///
/// The CI actor wins over the OS username: in CI the OS user is a service
/// account (`runner`, `root`) that identifies nobody, while the CI actor is the
/// person whose change triggered the run — which is what anyone reading a runs
/// list actually wants to know.
fn actor() -> Option<String> {
    for key in [
        "DOMARINN_ACTOR",
        "GITHUB_ACTOR",
        "GITLAB_USER_LOGIN",
        "BUILDKITE_BUILD_CREATOR",
        "BUILD_USER",
    ] {
        if let Some(value) = env(key) {
            return Some(value);
        }
    }
    whoami::username().ok()
}

fn host() -> Option<String> {
    if let Some(value) = env("DOMARINN_HOST") {
        return Some(value);
    }
    whoami::hostname().ok()
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    // `-C` rather than inheriting the process cwd: the engine is handed the
    // suite's directory, which is not necessarily where the CLI was invoked.
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Branch, commit and dirty state of the repo containing `dir`, or `None` when
/// it is not a repo.
///
/// Returns before running `status --porcelain` when neither rev-parse produced
/// anything, so a non-repo directory costs two cheap failing spawns rather than
/// three — and the expensive one, which walks the whole worktree, never runs.
pub fn collect_git(dir: &Path) -> Option<GitMeta> {
    let branch = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let commit = git(dir, &["rev-parse", "HEAD"]);
    if branch.is_none() && commit.is_none() {
        return None;
    }
    let dirty = git(dir, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    Some(GitMeta {
        branch,
        commit,
        dirty,
    })
}

/// Detect the CI system from the environment.
///
/// Note the bare `CI` fallback returns `Some`, so `ci.is_some()` — and the
/// server's `ci_provider IS NOT NULL` — is an exact "was this a CI run?"
/// predicate. That is why there is no separate boolean: a second field that
/// could disagree with this one would be a worse shape, not a better one.
pub fn collect_ci() -> Option<CiMeta> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses_its_documented_spellings() {
        assert_eq!(ProvenanceMode::parse("full"), Some(ProvenanceMode::Full));
        assert_eq!(ProvenanceMode::parse("  OFF "), Some(ProvenanceMode::Off));
        assert_eq!(
            ProvenanceMode::parse("Anonymous"),
            Some(ProvenanceMode::Anonymous)
        );
        assert_eq!(ProvenanceMode::parse("sometimes"), None);
    }

    #[test]
    fn off_records_nothing_at_all() {
        let collected = collect(
            &ProvenanceOptions {
                mode: ProvenanceMode::Off,
                note: Some("ignored".into()),
            },
            Path::new("."),
        );
        assert!(collected.origin.is_none());
        assert!(collected.git.is_none());
        assert!(collected.ci.is_none());
    }

    #[test]
    fn anonymous_drops_identity_but_keeps_the_rest_and_says_so() {
        let collected = collect(
            &ProvenanceOptions {
                mode: ProvenanceMode::Anonymous,
                note: Some("tuning retries".into()),
            },
            Path::new("."),
        );
        let origin = collected.origin.expect("anonymous still records an origin");
        assert_eq!(origin.actor, None);
        assert_eq!(origin.host, None);
        assert_eq!(origin.note.as_deref(), Some("tuning retries"));
        assert_eq!(origin.version.as_deref(), Some(crate::VERSION));
        // The positive marker is the whole point: without it, suppressed and
        // "written by an old client" are indistinguishable.
        assert_eq!(origin.redacted, Some(true));
    }

    #[test]
    fn full_leaves_redacted_unset_so_it_never_reaches_the_wire() {
        let collected = collect(&ProvenanceOptions::default(), Path::new("."));
        let origin = collected.origin.expect("full records an origin");
        assert_eq!(origin.redacted, None);
    }

    /// A directory that is not a repo must not be reported as a clean checkout.
    #[test]
    fn git_metadata_is_absent_outside_a_repository() {
        let dir = std::env::temp_dir().join("domarinn-provenance-not-a-repo");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(collect_git(&dir).is_none());
    }
}
