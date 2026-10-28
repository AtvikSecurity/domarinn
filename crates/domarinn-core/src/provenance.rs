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

/// Where a branch name comes from in CI, in the order we trust the answers.
///
/// Each entry is consulted in turn and the first one that normalizes to a real
/// branch wins; a variable holding a tag, a pull-request merge ref or an empty
/// string is skipped rather than accepted, so a tag build falls through to the
/// next candidate instead of recording `v1.2.3` as a branch.
///
/// The order within a provider is "most specific first": on a pull request the
/// *source* branch is the answer anyone wants, and the generic ref points at a
/// synthetic merge ref instead.
const BRANCH_ENV: &[&str] = &[
    // An explicit override wins over any autodetection, matching DOMARINN_ACTOR.
    "DOMARINN_BRANCH",
    // GitHub Actions — and Gitea/Forgejo Actions, whose runners set the same
    // names. `GITHUB_HEAD_REF` is set only on `pull_request` events and holds
    // the source branch; `GITHUB_REF` is then `refs/pull/N/merge`, which
    // `branch_name` rejects. `GITHUB_REF_NAME` is deliberately absent from this
    // list: it spells that same merge ref as `N/merge`, which looks like a
    // branch and is not.
    "GITHUB_HEAD_REF",
    "GITHUB_REF",
    // GitLab CI. `CI_COMMIT_BRANCH` is unset on tag and merge-request pipelines,
    // which is what makes it safe to take at face value.
    "CI_MERGE_REQUEST_SOURCE_BRANCH_NAME",
    "CI_COMMIT_BRANCH",
    // Buildkite, CircleCI, Travis, Drone/Woodpecker, Bitbucket, AppVeyor.
    "BUILDKITE_BRANCH",
    "CIRCLE_BRANCH",
    "TRAVIS_PULL_REQUEST_BRANCH",
    "TRAVIS_BRANCH",
    "DRONE_SOURCE_BRANCH",
    "DRONE_BRANCH",
    "BITBUCKET_BRANCH",
    "APPVEYOR_REPO_BRANCH",
    // Azure Pipelines. Both are full refs.
    "SYSTEM_PULLREQUEST_SOURCEBRANCH",
    "BUILD_SOURCEBRANCH",
    // Jenkins: `BRANCH_NAME` on a multibranch pipeline, `GIT_BRANCH` from the
    // git plugin — which spells it `origin/main`.
    "BRANCH_NAME",
    "GIT_BRANCH",
];

/// The branch CI says it is building, if any.
fn ci_branch() -> Option<String> {
    branch_from(env)
}

/// [`ci_branch`] with the environment injected, so the precedence rules are
/// testable without mutating process state — `std::env::set_var` is global to a
/// test binary whose tests run in parallel.
fn branch_from(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    BRANCH_ENV
        .iter()
        .filter_map(|key| get(key))
        .find_map(|raw| branch_name(&raw))
}

/// Reduce a ref to a branch name, or `None` when it does not name a branch.
///
/// Rejecting is as important as stripping: `refs/tags/v1.2.3` and
/// `refs/pull/42/merge` are refs a CI variable will happily hand over, and a
/// detached checkout reports the literal `HEAD` — none of which is a branch, and
/// all of which would otherwise be stored as one and then show up in the runs
/// list, the `?branch=` filter and the retention key.
fn branch_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let name = name
        .strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/remotes/origin/"))
        // Jenkins' `GIT_BRANCH`. A branch genuinely named `origin/…` would be
        // mangled here; that is a worse name than this is a bug.
        .or_else(|| name.strip_prefix("origin/"))
        .unwrap_or(name);
    if name.is_empty() || name == "HEAD" || name.starts_with("refs/") {
        return None;
    }
    Some(name.to_string())
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
///
/// **The branch comes from CI first**, for the same reason the actor does: CI
/// checks out a detached HEAD, so `rev-parse --abbrev-ref` answers the literal
/// `HEAD` for every run on a runner. That is not a branch, it is an artifact of
/// how the checkout works — and it was being stored as one, which is what a
/// `?branch=` filter, a `--against` comparison and the server's
/// `(project, suite, branch)` retention key all read.
///
/// Whether this is a repo at all stays a question only git answers. Deciding it
/// from the environment would make a run in a non-repo directory on a runner
/// report a branch and no commit, which is a checkout that never existed.
pub fn collect_git(dir: &Path) -> Option<GitMeta> {
    let head = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let commit = git(dir, &["rev-parse", "HEAD"]);
    if head.is_none() && commit.is_none() {
        return None;
    }
    let branch = ci_branch().or_else(|| head.as_deref().and_then(branch_name));
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
    ci_from(env)
}

/// [`collect_ci`] with the environment injected, for the same reason
/// [`branch_from`] takes one: the detection order is the thing worth testing,
/// and `std::env::set_var` is global to a test binary that runs in parallel.
///
/// **Gitea and Forgejo come first** because their runners also set
/// `GITHUB_ACTIONS=true` — that is how a GitHub-shaped workflow runs there
/// unchanged — so checked after it they would be recorded as GitHub, with a
/// run URL built the GitHub way. On these forges that URL is wrong:
///
/// - **Forgejo** addresses a run page by the repository's run *number*
///   (`GITHUB_RUN_NUMBER`, the per-repo index), not the instance-wide
///   `GITHUB_RUN_ID`. Built from the id, the link opens a different run of the
///   same repository, or 404s.
/// - **Gitea** used the run number in the URL up to 1.25 and the id from 1.26,
///   where the run page still accepts a number when asked with `?by_index=1`
///   (an older server ignores the unknown query). Number plus `by_index` is
///   the one spelling that opens the right run on both sides of that change;
///   the id alone 404s or lands on a different run on every Gitea before 1.26.
///
/// `GITHUB_SERVER_URL` is trimmed of a trailing slash: Gitea derives it from
/// its `ROOT_URL`, which it stores with one, and `host//owner/repo` is a link
/// that only works by accident.
///
/// The rest hand over a direct link, so they are taken at face value. A
/// system whose link would have to be assembled from parts (Azure Pipelines,
/// Bitbucket) is left to the bare `CI` fallback rather than guessed at.
/// Woodpecker is matched on `CI=woodpecker` and checked before Drone, whose
/// variable names older Woodpecker releases also set.
fn ci_from(get: impl Fn(&str) -> Option<String>) -> Option<CiMeta> {
    let found = |provider: &str, run_url: Option<String>| {
        Some(CiMeta {
            provider: Some(provider.into()),
            run_url,
        })
    };
    let actions_run = |run: Option<String>, query: &str| match (
        get("GITHUB_SERVER_URL"),
        get("GITHUB_REPOSITORY"),
        run,
    ) {
        (Some(server), Some(repo), Some(run)) => Some(format!(
            "{}/{repo}/actions/runs/{run}{query}",
            server.trim_end_matches('/')
        )),
        _ => None,
    };
    if get("FORGEJO_ACTIONS").is_some() {
        return found("forgejo", actions_run(get("GITHUB_RUN_NUMBER"), ""));
    }
    if get("GITEA_ACTIONS").is_some() {
        return found(
            "gitea",
            actions_run(get("GITHUB_RUN_NUMBER"), "?by_index=1"),
        );
    }
    if get("GITHUB_ACTIONS").is_some() {
        return found("github", actions_run(get("GITHUB_RUN_ID"), ""));
    }
    // Each marker, and the variable holding that system's link to this run.
    for (marker, provider, url) in [
        ("GITLAB_CI", "gitlab", "CI_JOB_URL"),
        ("BUILDKITE", "buildkite", "BUILDKITE_BUILD_URL"),
        ("CIRCLECI", "circleci", "CIRCLE_BUILD_URL"),
        ("TRAVIS", "travis", "TRAVIS_BUILD_WEB_URL"),
    ] {
        if get(marker).is_some() {
            return found(provider, get(url));
        }
    }
    if get("CI").as_deref() == Some("woodpecker") {
        return found("woodpecker", get("CI_PIPELINE_URL"));
    }
    if get("DRONE").is_some() {
        return found("drone", get("DRONE_BUILD_LINK"));
    }
    if get("JENKINS_URL").is_some() {
        return found("jenkins", get("BUILD_URL"));
    }
    if get("CI").is_some() {
        return found("ci", None);
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

    /// Look up `key` in a fixture table, the way [`branch_from`] looks it up in
    /// the environment.
    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
                .filter(|v| !v.is_empty())
        }
    }

    #[test]
    fn a_ref_is_reduced_to_a_branch_name() {
        assert_eq!(branch_name("refs/heads/main").as_deref(), Some("main"));
        assert_eq!(
            branch_name("refs/heads/feat/ci-branch").as_deref(),
            Some("feat/ci-branch")
        );
        assert_eq!(branch_name("origin/main").as_deref(), Some("main"));
        assert_eq!(
            branch_name("refs/remotes/origin/main").as_deref(),
            Some("main")
        );
        assert_eq!(branch_name(" main \n").as_deref(), Some("main"));
    }

    /// The rejections are the point: each of these would otherwise be stored as
    /// a branch and then filtered on as one.
    #[test]
    fn what_does_not_name_a_branch_is_refused() {
        assert_eq!(branch_name("HEAD"), None, "a detached checkout");
        assert_eq!(branch_name("refs/tags/v1.2.3"), None, "a tag build");
        assert_eq!(branch_name("refs/pull/42/merge"), None, "a merge ref");
        assert_eq!(branch_name("   "), None);
    }

    /// On a pull request the source branch is the answer; `GITHUB_REF` points at
    /// the synthetic merge ref, which is nobody's branch.
    #[test]
    fn a_github_pull_request_records_the_source_branch() {
        let vars = [
            ("GITHUB_HEAD_REF", "feat/ci-branch"),
            ("GITHUB_REF", "refs/pull/42/merge"),
        ];
        assert_eq!(
            branch_from(lookup(&vars)).as_deref(),
            Some("feat/ci-branch")
        );
    }

    /// On a push `GITHUB_HEAD_REF` is defined but empty, and the ref is real.
    #[test]
    fn a_github_push_falls_through_the_empty_head_ref() {
        let vars = [("GITHUB_HEAD_REF", ""), ("GITHUB_REF", "refs/heads/main")];
        assert_eq!(branch_from(lookup(&vars)).as_deref(), Some("main"));
    }

    /// A tag build has no branch. Falling through to a later provider's variable
    /// would be worse than admitting that.
    #[test]
    fn a_tag_build_reports_no_branch() {
        let vars = [("GITHUB_HEAD_REF", ""), ("GITHUB_REF", "refs/tags/v0.6.1")];
        assert_eq!(branch_from(lookup(&vars)), None);
    }

    #[test]
    fn an_explicit_override_beats_every_detected_value() {
        let vars = [
            ("DOMARINN_BRANCH", "release/2026-07"),
            ("GITHUB_REF", "refs/heads/main"),
        ];
        assert_eq!(
            branch_from(lookup(&vars)).as_deref(),
            Some("release/2026-07")
        );
    }

    #[test]
    fn the_other_forges_are_understood_too() {
        for (key, raw, want) in [
            ("CI_MERGE_REQUEST_SOURCE_BRANCH_NAME", "feat/x", "feat/x"),
            ("CI_COMMIT_BRANCH", "main", "main"),
            ("BUILDKITE_BRANCH", "main", "main"),
            ("CIRCLE_BRANCH", "main", "main"),
            ("BUILD_SOURCEBRANCH", "refs/heads/main", "main"),
            ("GIT_BRANCH", "origin/main", "main"),
        ] {
            let vars = [(key, raw)];
            assert_eq!(
                branch_from(lookup(&vars)).as_deref(),
                Some(want),
                "{key} = {raw}"
            );
        }
    }

    #[test]
    fn nothing_in_the_environment_means_no_ci_branch() {
        assert_eq!(branch_from(|_| None), None);
    }

    /// GitHub's run page is addressed by the instance-wide run id.
    #[test]
    fn a_github_run_links_by_run_id() {
        let vars = [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://github.com"),
            ("GITHUB_REPOSITORY", "acme/widgets"),
            ("GITHUB_RUN_ID", "9001"),
            ("GITHUB_RUN_NUMBER", "42"),
        ];
        let ci = ci_from(lookup(&vars)).expect("a CI run");
        assert_eq!(ci.provider.as_deref(), Some("github"));
        assert_eq!(
            ci.run_url.as_deref(),
            Some("https://github.com/acme/widgets/actions/runs/9001")
        );
    }

    /// Forgejo's runner sets `GITHUB_ACTIONS=true` too, so the forge marker has
    /// to win — and its run page is the per-repo run *number*. The GitHub-style
    /// link built from `GITHUB_RUN_ID` opens a different run or none.
    #[test]
    fn a_forgejo_run_links_by_run_number_not_run_id() {
        let vars = [
            ("FORGEJO_ACTIONS", "true"),
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://code.example/"),
            ("GITHUB_REPOSITORY", "acme/widgets"),
            ("GITHUB_RUN_ID", "117195"),
            ("GITHUB_RUN_NUMBER", "9384"),
        ];
        let ci = ci_from(lookup(&vars)).expect("a CI run");
        assert_eq!(ci.provider.as_deref(), Some("forgejo"));
        assert_eq!(
            ci.run_url.as_deref(),
            Some("https://code.example/acme/widgets/actions/runs/9384"),
            "the trailing slash on the server URL must not double up either"
        );
    }

    /// Gitea switched its run URLs from number to id in 1.26 and kept the
    /// number reachable behind `?by_index=1`; an older server ignores the
    /// query and reads the path as a number anyway. Either way, the number is
    /// the run this job is.
    #[test]
    fn a_gitea_run_links_by_run_number_on_every_gitea_version() {
        let vars = [
            ("GITEA_ACTIONS", "true"),
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://gitea.example/"),
            ("GITHUB_REPOSITORY", "acme/widgets"),
            ("GITHUB_RUN_ID", "5521"),
            ("GITHUB_RUN_NUMBER", "17"),
        ];
        let ci = ci_from(lookup(&vars)).expect("a CI run");
        assert_eq!(ci.provider.as_deref(), Some("gitea"));
        assert_eq!(
            ci.run_url.as_deref(),
            Some("https://gitea.example/acme/widgets/actions/runs/17?by_index=1")
        );
    }

    /// Forgejo began as a Gitea fork; a runner that sets both markers is a
    /// Forgejo runner, and the two forges address runs differently.
    #[test]
    fn forgejo_wins_over_gitea_when_both_markers_are_set() {
        let vars = [
            ("FORGEJO_ACTIONS", "true"),
            ("GITEA_ACTIONS", "true"),
            ("GITHUB_ACTIONS", "true"),
        ];
        let ci = ci_from(lookup(&vars)).expect("a CI run");
        assert_eq!(ci.provider.as_deref(), Some("forgejo"));
        assert_eq!(ci.run_url, None, "no server, repo or number: no link");
    }

    /// The systems that hand over a direct link get it recorded, not just a
    /// provider name. Woodpecker is told apart by `CI=woodpecker`, which also
    /// keeps it from falling through to the anonymous `ci` bucket.
    #[test]
    fn the_other_ci_systems_record_their_run_link() {
        for (vars, provider, url) in [
            (
                &[("GITLAB_CI", "true"), ("CI_JOB_URL", "https://gl/j/1")][..],
                "gitlab",
                "https://gl/j/1",
            ),
            (
                &[
                    ("BUILDKITE", "true"),
                    ("BUILDKITE_BUILD_URL", "https://bk/b/2"),
                ][..],
                "buildkite",
                "https://bk/b/2",
            ),
            (
                &[("CIRCLECI", "true"), ("CIRCLE_BUILD_URL", "https://cc/b/3")][..],
                "circleci",
                "https://cc/b/3",
            ),
            (
                &[
                    ("TRAVIS", "true"),
                    ("TRAVIS_BUILD_WEB_URL", "https://tr/b/4"),
                ][..],
                "travis",
                "https://tr/b/4",
            ),
            (
                &[
                    ("CI", "woodpecker"),
                    // Older Woodpecker releases also set Drone's names.
                    ("DRONE", "true"),
                    ("DRONE_BUILD_LINK", "https://wp/wrong"),
                    ("CI_PIPELINE_URL", "https://wp/repos/7/pipeline/8"),
                ][..],
                "woodpecker",
                "https://wp/repos/7/pipeline/8",
            ),
            (
                &[("DRONE", "true"), ("DRONE_BUILD_LINK", "https://dr/o/r/42")][..],
                "drone",
                "https://dr/o/r/42",
            ),
            (
                &[
                    ("JENKINS_URL", "https://j/"),
                    ("BUILD_URL", "https://j/job/5"),
                ][..],
                "jenkins",
                "https://j/job/5",
            ),
        ] {
            let ci = ci_from(lookup(vars)).unwrap_or_else(|| panic!("{provider}: a CI run"));
            assert_eq!(ci.provider.as_deref(), Some(provider));
            assert_eq!(ci.run_url.as_deref(), Some(url), "{provider}");
        }
    }

    /// A bare `CI` still marks a CI run — `ci.is_some()` is the server's
    /// "was this CI?" predicate — just with nothing to link to.
    #[test]
    fn a_bare_ci_marker_is_a_ci_run_without_a_link() {
        let ci = ci_from(lookup(&[("CI", "true")])).expect("a CI run");
        assert_eq!(ci.provider.as_deref(), Some("ci"));
        assert_eq!(ci.run_url, None);
        assert!(ci_from(|_| None).is_none());
    }

    /// A directory that is not a repo must not be reported as a clean checkout.
    #[test]
    fn git_metadata_is_absent_outside_a_repository() {
        let dir = std::env::temp_dir().join("domarinn-provenance-not-a-repo");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(collect_git(&dir).is_none());
    }
}
