//! The Gitea and Forgejo copies of the action: what they may differ from the
//! GitHub one in, and the PR-comment step only they have.
//!
//! # Why the copies are policed
//!
//! Three files carrying the same shell is three chances for a fix to land in
//! one and not the others — and nothing here runs a Gitea or Forgejo runner, so
//! a copy that drifted would keep passing until a user on that forge hit the
//! bug the other two had fixed. The parent file runs every contract test
//! against all three copies; [`the_shared_steps_are_identical_in_every_copy`]
//! goes further and holds the shared steps to byte equality, so the one place
//! a difference *can* live is a step this file names and explains.
//!
//! # The documented differences
//!
//! - **Upload results** uses an artifact action that runs off github.com, and
//!   is `continue-on-error`. Its inputs and its condition are the GitHub step's.
//! - **Comment on PR** is a `run:` step (REST create-or-update in node) rather
//!   than `actions/github-script`, and also fires on `pull_request_target`.
//! - **Inputs**: the forge copies add [`FORGE_ONLY_INPUTS`]; everything else is
//!   declared with the same defaults. **Outputs** are identical.
//! - The header comment and the one-line `description`.
//!
//! The Gitea and Forgejo copies differ from *each other* only in the upload
//! step's `uses:` and the description.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::{
    parsed, repo_root, run_step, steps, Workspace, ACTIONS, FORGEJO_ACTION, FORGE_ONLY_INPUTS,
    GITEA_ACTION, GITHUB_ACTION, SHELL_STEPS,
};

const FORGE_ACTIONS: &[&str] = &[GITEA_ACTION, FORGEJO_ACTION];

/// The step named `name` in `action`.
fn named(action: &str, name: &str) -> Value {
    steps(action)
        .into_iter()
        .find(|s| s["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("{action} has no step named {name:?}"))
}

/// The shared steps — binary, eval, summary, gate — are the same YAML in every
/// copy: name, id, condition, shell, environment and script, compared as
/// parsed values so only a difference in meaning (not in indentation) counts.
#[test]
fn the_shared_steps_are_identical_in_every_copy() {
    for shared in SHELL_STEPS {
        let reference = super::step(GITHUB_ACTION, shared.field, shared.value);
        for action in FORGE_ACTIONS {
            let copy = super::step(action, shared.field, shared.value);
            assert_eq!(
                copy, reference,
                "{action}: the `{}` step has drifted from {GITHUB_ACTION}. The \
                 shared steps are copied byte for byte — edit all three, or, if \
                 the forge genuinely needs something different, move the \
                 difference out of the shared step and document it in this file.",
                shared.value
            );
        }
    }
}

/// Same steps, same order. A step added to one copy and not the others is
/// either a feature one forge silently lacks or an unreviewed difference.
#[test]
fn every_copy_runs_the_same_steps_in_the_same_order() {
    let names = |action: &str| -> Vec<String> {
        steps(action)
            .iter()
            .map(|s| s["name"].as_str().expect("every step is named").to_string())
            .collect()
    };
    let reference = names(GITHUB_ACTION);
    for action in FORGE_ACTIONS {
        assert_eq!(names(action), reference, "{action}");
    }
}

/// The upload step may swap the action it `uses:` and must tolerate a missing
/// artifact backend; it may not upload something else, or under another
/// condition, than the GitHub step does.
#[test]
fn the_upload_step_differs_only_in_the_action_it_uses() {
    let reference = named(GITHUB_ACTION, "Upload results");
    for action in FORGE_ACTIONS {
        let copy = named(action, "Upload results");
        assert_eq!(copy["if"], reference["if"], "{action}: upload condition");
        assert_eq!(copy["with"], reference["with"], "{action}: upload inputs");
        assert_eq!(
            copy["continue-on-error"],
            Value::Bool(true),
            "{action}: an instance with artifact storage disabled must lose the \
             artifact, not the verdict"
        );
        let uses = copy["uses"]
            .as_str()
            .expect("the upload step uses an action");
        // The SHA says nothing about the version; the pin comment beside it
        // does, and Renovate keeps that comment true when it moves the SHA.
        let raw = super::raw_action(action);
        let pinned = raw
            .lines()
            .find(|l| l.contains(&format!("uses: {uses}")))
            .unwrap_or_else(|| panic!("{action}: no `uses: {uses}` line"));
        assert!(
            !uses.starts_with("actions/upload-artifact@") || pinned.contains("# v3."),
            "{action}: upstream upload-artifact v4+ refuses every host that is \
             not github.com; got {pinned}"
        );
    }
}

/// Gitea and Forgejo diverge from each other in the upload action alone. With
/// that and the description set aside, the two parsed files are equal — which
/// covers the comment step, the inputs and the outputs in one assertion.
#[test]
fn the_gitea_and_forgejo_copies_differ_only_in_the_upload_action() {
    let normalized = |action: &str| {
        let mut doc = parsed(action);
        doc.as_object_mut().unwrap().remove("description");
        for step in doc["runs"]["steps"].as_array_mut().unwrap() {
            if step["name"] == "Upload results" {
                step.as_object_mut().unwrap().remove("uses");
            }
        }
        doc
    };
    assert_eq!(normalized(GITEA_ACTION), normalized(FORGEJO_ACTION));
}

/// Every input the GitHub action takes, the forge copies take with the same
/// default; they add only [`FORGE_ONLY_INPUTS`]. Outputs match exactly — a
/// workflow ported between forges must not lose an output it reads.
#[test]
fn every_copy_declares_the_same_inputs_and_outputs() {
    let defaults = |action: &str| -> BTreeMap<String, Value> {
        parsed(action)["inputs"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v["default"].clone()))
            .collect()
    };
    let reference = defaults(GITHUB_ACTION);
    let reference_outputs = parsed(GITHUB_ACTION)["outputs"].clone();
    for action in FORGE_ACTIONS {
        let mut copy = defaults(action);
        for extra in FORGE_ONLY_INPUTS {
            assert!(
                copy.remove(*extra).is_some(),
                "{action}: declares no `{extra}` input"
            );
        }
        assert_eq!(
            copy, reference,
            "{action}: inputs or their defaults drifted"
        );
        assert_eq!(
            parsed(action)["outputs"],
            reference_outputs,
            "{action}: outputs drifted"
        );
    }
}

/// Every `uses:` is pinned to a full commit SHA, as the GitHub copy's are. A
/// tag can be moved under a workflow; a SHA cannot.
#[test]
fn every_action_reference_is_pinned_to_a_commit() {
    for action in ACTIONS {
        for step in steps(action) {
            let Some(uses) = step["uses"].as_str() else {
                continue;
            };
            let sha = uses.rsplit_once('@').map(|(_, r)| r).unwrap_or_default();
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{action}: `{uses}` is not pinned to a full commit SHA"
            );
        }
    }
}

/// Only `actions/` may exist under `.gitea/` and `.forgejo/`. A `workflows/`
/// directory there makes Gitea (and Forgejo) run *those* workflows and ignore
/// `.github/workflows` — so on any mirror of this repository, CI would quietly
/// stop being the CI this repository defines.
#[test]
fn no_forge_workflows_directory_shadows_the_github_one() {
    for forge in [".gitea", ".forgejo"] {
        let dir = repo_root().join(forge).join("workflows");
        assert!(
            !dir.exists(),
            "{} exists; it would replace .github/workflows on a mirror",
            dir.display()
        );
    }
}

// ── The PR comment step ────────────────────────────────────────────────────

/// One request the stub forge received.
#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    auth: Option<String>,
    body: String,
}

type Handler = dyn Fn(&str, &str) -> (u16, String) + Send + Sync;

/// A throwaway HTTP server standing in for the forge's API. It answers each
/// request from `handler(method, path)` and closes the connection, which is
/// all node's client needs. The thread outlives the test and dies with the
/// binary; nothing joins it, so a hang can only come from the step itself.
struct StubForge {
    api_url: String,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl StubForge {
    fn start(handler: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let api_url = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let handler: Arc<Handler> = Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let (mut length, mut auth) = (0usize, None);
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                    let (name, value) = header.split_once(':').unwrap_or_default();
                    match name.to_ascii_lowercase().as_str() {
                        "content-length" => length = value.trim().parse().unwrap_or(0),
                        "authorization" => auth = Some(value.trim().to_string()),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let (status, reply) = handler(&method, &path);
                log.lock().unwrap().push(Request {
                    method,
                    path,
                    auth,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self { api_url, seen }
    }

    fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

/// Run the comment step of `action` against `forge`, with a summary on disk.
fn comment_against(action: &str, forge: &StubForge) -> (super::Ran, Vec<Request>) {
    assert!(
        std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success()),
        "these tests run the step's real node script, so they need `node` on \
         PATH (`mise install` provides it)"
    );
    let ws = Workspace::new();
    std::fs::write(
        ws.path().join("summary.md"),
        "### domarinn eval\n\n12 passed\n",
    )
    .unwrap();
    let ran = run_step(action, "comment", &[("API_URL", &forge.api_url)], &ws);
    (ran, forge.requests())
}

fn comments_json(comments: &[(u64, &str)]) -> String {
    serde_json::to_string(
        &comments
            .iter()
            .map(|(id, body)| serde_json::json!({ "id": id, "body": body }))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

/// First run on a PR: nothing carries the marker, so one comment is created —
/// with the marker, the summary, and the token in the forge's header form.
#[test]
fn the_comment_step_creates_a_comment_when_none_is_marked() {
    for action in FORGE_ACTIONS {
        let forge = StubForge::start(|method, _| match method {
            "GET" => (200, comments_json(&[(3, "LGTM")])),
            "POST" => (201, r#"{"id":12}"#.into()),
            _ => (500, "{}".into()),
        });
        let (ran, seen) = comment_against(action, &forge);

        assert_eq!(ran.status, Some(0), "{action}: {}", ran.log);
        let post = seen
            .iter()
            .find(|r| r.method == "POST")
            .unwrap_or_else(|| panic!("{action}: no comment was created: {seen:?}\n{}", ran.log));
        assert_eq!(post.path, "/api/v1/repos/acme/widgets/issues/7/comments");
        assert_eq!(post.auth.as_deref(), Some("token t0ken"));
        let body: Value = serde_json::from_str(&post.body).unwrap();
        let text = body["body"].as_str().unwrap();
        assert!(
            text.starts_with("<!-- domarinn-eval -->\n") && text.contains("12 passed"),
            "{action}: got {text:?}"
        );
        assert!(
            seen.iter().all(|r| r.method != "PATCH"),
            "{action}: {seen:?}"
        );
    }
}

/// A later push updates the marked comment in place instead of adding another.
#[test]
fn the_comment_step_updates_the_marked_comment() {
    for action in FORGE_ACTIONS {
        let forge = StubForge::start(|method, _| match method {
            "GET" => (
                200,
                comments_json(&[(3, "LGTM"), (11, "<!-- domarinn-eval -->\nold")]),
            ),
            "PATCH" => (200, r#"{"id":11}"#.into()),
            _ => (500, "{}".into()),
        });
        let (ran, seen) = comment_against(action, &forge);

        assert_eq!(ran.status, Some(0), "{action}: {}", ran.log);
        let patch = seen
            .iter()
            .find(|r| r.method == "PATCH")
            .unwrap_or_else(|| panic!("{action}: nothing was updated: {seen:?}\n{}", ran.log));
        assert_eq!(patch.path, "/api/v1/repos/acme/widgets/issues/comments/11");
        assert!(patch.body.contains("12 passed"), "{action}: {}", patch.body);
        assert!(
            seen.iter().all(|r| r.method != "POST"),
            "{action}: {seen:?}"
        );
    }
}

/// Gitea and Forgejo ignore `page`/`limit` on this route today and return the
/// whole list every time. A walk that only stopped on a short page would then
/// request page after page of the same comments forever; this one stops at
/// the first page with nothing new.
#[test]
fn the_comment_step_stops_paging_when_the_forge_ignores_page() {
    for action in FORGE_ACTIONS {
        let forge = StubForge::start(|method, _| match method {
            "GET" => {
                let full: Vec<(u64, &str)> = (1..=50).map(|id| (id, "chatter")).collect();
                (200, comments_json(&full))
            }
            "POST" => (201, r#"{"id":99}"#.into()),
            _ => (500, "{}".into()),
        });
        let (ran, seen) = comment_against(action, &forge);

        assert_eq!(ran.status, Some(0), "{action}: {}", ran.log);
        let gets = seen.iter().filter(|r| r.method == "GET").count();
        assert_eq!(
            gets, 2,
            "{action}: a full first page asks for a second, and a second with no \
             new ids ends it: {seen:?}"
        );
        assert_eq!(seen.iter().filter(|r| r.method == "POST").count(), 1);
    }
}

/// A marked comment the token may not edit — someone quoted the marker, say —
/// is not the end of it: the step posts a fresh comment instead.
#[test]
fn the_comment_step_posts_anew_when_the_update_is_refused() {
    for action in FORGE_ACTIONS {
        let forge = StubForge::start(|method, _| match method {
            "GET" => (200, comments_json(&[(5, "> <!-- domarinn-eval -->")])),
            "PATCH" => (403, r#"{"message":"forbidden"}"#.into()),
            "POST" => (201, r#"{"id":6}"#.into()),
            _ => (500, "{}".into()),
        });
        let (ran, seen) = comment_against(action, &forge);

        assert_eq!(ran.status, Some(0), "{action}: {}", ran.log);
        assert!(ran.log.contains("::warning::could not update comment 5"));
        assert_eq!(
            seen.iter().filter(|r| r.method == "POST").count(),
            1,
            "{action}: {seen:?}"
        );
    }
}

/// A token without write access — a fork's pull request, the usual case — is a
/// warning. The gate step owns the verdict; the comment never fails the job.
#[test]
fn a_refused_comment_is_a_warning_never_a_failure() {
    for action in FORGE_ACTIONS {
        let forge = StubForge::start(|_, _| (403, r#"{"message":"token has no write"}"#.into()));
        let (ran, _) = comment_against(action, &forge);

        assert_eq!(ran.status, Some(0), "{action}: {}", ran.log);
        assert!(
            ran.log.contains("::warning::PR comment failed") && ran.log.contains("HTTP 403"),
            "{action}: the warning must say what the forge answered, got:\n{}",
            ran.log
        );
    }
}

/// With no API URL there is nothing to call. The step says so and succeeds,
/// and in particular does not go looking for `undefined/repos/...`.
#[test]
fn the_comment_step_skips_cleanly_without_an_api_url() {
    for action in FORGE_ACTIONS {
        let ws = Workspace::new();
        let ran = run_step(action, "comment", &[], &ws);
        assert_eq!(ran.status, Some(0), "{action}: {}", ran.log);
        assert!(
            ran.log.contains("::warning::PR comment skipped"),
            "{action}: got:\n{}",
            ran.log
        );
    }
}
