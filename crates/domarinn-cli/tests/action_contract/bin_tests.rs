//! The binary-resolution step: provided path, then release download, then a
//! source build — and what each of its dead ends reports. Split from
//! `action_contract.rs` (whose `run_step`/`Workspace` harness these use via
//! `super::`) to keep that file under the 1000-line cap.

use super::{run_step, Workspace, ACTIONS};

/// `version: latest` has to become a concrete tag before a download URL
/// exists, and the redirect it reads that from is a network call that can
/// fail. When it does, the step must say so and exit 3 — the same
/// infrastructure code its other two dead ends already use.
///
/// The abort this catches is quieter than the eval step's: the CLI never runs,
/// so there is no run to summarize and no report to upload, and the operator
/// gets a red step whose only content is the shell's own exit status.
#[test]
fn an_unresolvable_latest_release_is_an_infrastructure_error() {
    for action in ACTIONS {
        for base in [
            "https://github.com/AtvikSecurity/domarinn",
            "https://forge.example/mirrors/domarinn",
        ] {
            let ws = Workspace::new();
            // What `curl -f` does for a 404, and near enough what it does for
            // a DNS or TLS failure: a non-zero exit and nothing on stdout.
            ws.shim("curl", "exit 22\n");

            let ran = run_step(
                action,
                "bin",
                &[
                    ("INPUT_VERSION", "latest"),
                    ("INPUT_RELEASE_BASE_URL", base),
                ],
                &ws,
            );

            assert!(
                ran.log.contains("::error::"),
                "{action} ({base}): a failed release lookup must be annotated, \
                 not left to the shell; the step logged:\n{}",
                ran.log
            );
            assert!(
                ran.log
                    .contains("could not resolve the latest domarinn release tag"),
                "{action} ({base}): the annotation must name what failed, got:\n{}",
                ran.log
            );
            assert_eq!(
                ran.status,
                Some(3),
                "{action} ({base}): the action documents 3 for an infrastructure \
                 error; curl's own exit code is not one of the four the gate \
                 step knows how to render"
            );
        }
    }
}

/// A mirror is not github.com, so `latest` cannot be read off GitHub's
/// `/releases/latest` redirect there. It is asked of the Gitea/Forgejo release
/// API instead — at the instance root, which is the base URL minus its
/// `owner/repo`, so a forge served under a path prefix still works — and the
/// asset is then downloaded from the mirror, never from github.com.
#[test]
fn a_mirror_resolves_latest_through_the_forge_api_and_downloads_from_itself() {
    for action in ACTIONS {
        let ws = Workspace::new();
        ws.shim("uname", "echo x86_64\n");
        // Log every invocation; answer the API with a compact release object
        // (Gitea's shape: no spaces), and "download" by creating the file.
        ws.shim(
            "curl",
            r#"echo "curl $*" >> curl.log
for a in "$@"; do
  case "$a" in
    */api/v1/repos/mirrors/domarinn/releases/latest)
      printf '{"id":3,"tag_name":"0.9.1","name":"0.9.1","draft":false}'; exit 0 ;;
  esac
done
prev=""
for a in "$@"; do
  if [ "$prev" = "-o" ]; then : > "$a"; exit 0; fi
  prev="$a"
done
exit 22
"#,
        );

        let ran = run_step(
            action,
            "bin",
            &[
                ("INPUT_VERSION", "latest"),
                (
                    "INPUT_RELEASE_BASE_URL",
                    "https://forge.example/git/mirrors/domarinn/",
                ),
            ],
            &ws,
        );
        let calls = std::fs::read_to_string(ws.path().join("curl.log")).unwrap_or_default();

        assert_eq!(
            ran.status,
            Some(0),
            "{action}: the step logged:\n{}\ncurl saw:\n{calls}",
            ran.log
        );
        assert!(
            calls.contains(
                "https://forge.example/git/api/v1/repos/mirrors/domarinn/releases/latest"
            ),
            "{action}: latest must be asked of the forge API at the instance \
             root (path prefix kept, trailing slash dropped); curl saw:\n{calls}"
        );
        assert!(
            calls.contains(
                "https://forge.example/git/mirrors/domarinn/releases/download/0.9.1/domarinn_0.9.1_linux_amd64"
            ),
            "{action}: the asset must come from the mirror; curl saw:\n{calls}"
        );
        assert!(
            !calls.contains("github.com"),
            "{action}: an air-gapped runner must never be sent to github.com; \
             curl saw:\n{calls}"
        );
        assert!(
            ran.output("path").is_some_and(|p| p.ends_with("/domarinn")),
            "{action}: got {:?}",
            ran.output("path")
        );
    }
}

/// An arch with no release asset is the binary step's other infrastructure
/// dead end. It already behaves; it is here so a fix to the `latest` path
/// cannot regress it.
#[test]
fn an_unsupported_architecture_is_an_infrastructure_error() {
    for action in ACTIONS {
        let ws = Workspace::new();
        ws.shim("uname", "echo riscv64\n");

        let ran = run_step(action, "bin", &[], &ws);

        assert_eq!(ran.status, Some(3), "{action}");
        assert!(
            ran.log.contains("::error::unsupported arch riscv64"),
            "{action}: got:\n{}",
            ran.log
        );
    }
}

/// A concrete version with no downloadable asset is *not* an error: the step
/// falls back to building from source. That tolerance comes from wrapping the
/// download in `if curl …`, which is the shape the `latest` lookup lacks — so
/// this pins the working half of the same step. The build clones the same
/// repository the releases came from, so a mirror serves both.
#[test]
fn a_missing_release_asset_falls_back_to_a_source_build() {
    for action in ACTIONS {
        let ws = Workspace::new();
        ws.shim("curl", "exit 22\n");
        ws.shim("cargo", "echo \"cargo $*\" >> cargo.log\nexit 0\n");

        let ran = run_step(
            action,
            "bin",
            &[
                ("INPUT_VERSION", "0.9.9"),
                (
                    "INPUT_RELEASE_BASE_URL",
                    "https://forge.example/mirrors/domarinn",
                ),
            ],
            &ws,
        );

        assert_eq!(
            ran.status,
            Some(0),
            "{action}: a missing asset is recoverable; the step logged:\n{}",
            ran.log
        );
        assert!(
            ran.log.contains("::warning::no release asset for 0.9.9"),
            "{action}: the fallback must be visible in the log, got:\n{}",
            ran.log
        );
        assert!(
            ran.output("path")
                .is_some_and(|p| p.ends_with("/bin/domarinn")),
            "{action}: the built binary's path is what every later step \
             invokes, got: {:?}",
            ran.output("path")
        );
        let cargo = std::fs::read_to_string(ws.path().join("cargo.log")).unwrap_or_default();
        assert!(
            cargo.contains("--git https://forge.example/mirrors/domarinn "),
            "{action}: the source build must clone the configured repository, \
             got: {cargo}"
        );
    }
}
