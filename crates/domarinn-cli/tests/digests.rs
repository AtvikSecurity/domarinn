//! Tests for component and per-case digests — the data that lets a comparison
//! say *what* changed rather than only *that* something did.

mod common;

use assert_cmd::prelude::*;
use common::{bin, latest_run};

/// A suite with a real prompt template, so a prompt edit is expressible.
fn suite(prompt: &str) -> String {
    format!(
        r#"
version: 1
project: p
suite: s
providers:
  - id: prov
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"hello\"}}'"]
prompts:
  - id: main
    template: "{prompt}"
tests:
  - id: t1
    vars: {{name: world}}
    assert:
      - {{type: contains, value: "hello"}}
"#
    )
}

fn write(dir: &std::path::Path, body: String) {
    std::fs::write(dir.join("domarinn.yaml"), body).unwrap();
}

#[test]
fn a_run_records_component_digests_for_every_part_of_the_suite() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), suite("greet {{ name }}"));
    bin().arg("run").current_dir(dir.path()).assert().success();

    let d = latest_run(dir.path())
        .digests
        .expect("a run records component digests");
    for (name, value) in [
        ("prompts", &d.prompts),
        ("providers", &d.providers),
        ("tests", &d.tests),
        ("asserts", &d.asserts),
        ("grader", &d.grader),
    ] {
        let v = value.as_deref().unwrap_or_else(|| panic!("{name} digest"));
        assert!(v.starts_with("blake3:"), "{name} should be blake3: {v}");
    }
}

/// The payoff: an edited prompt moves the prompts digest and leaves the others
/// alone, so a reader can say "the prompts changed" instead of "config changed".
#[test]
fn editing_a_prompt_moves_only_the_prompts_digest() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), suite("greet {{ name }}"));
    bin().arg("run").current_dir(dir.path()).assert().success();
    let before = latest_run(dir.path()).digests.unwrap();

    write(dir.path(), suite("say hi to {{ name }}"));
    bin()
        .args(["run", "--no-cache"])
        .current_dir(dir.path())
        .assert()
        .success();
    let after = latest_run(dir.path()).digests.unwrap();

    assert_ne!(before.prompts, after.prompts);
    assert_eq!(before.providers, after.providers);
    assert_eq!(before.tests, after.tests);
    assert_eq!(before.asserts, after.asserts);
}

/// Per-case identity must not depend on caching. The cache key — which these
/// digests were nearly derived from — is skipped entirely under `--no-cache`,
/// which is exactly the mode a CI comparison runs in.
#[test]
fn per_case_digests_are_recorded_even_with_the_cache_disabled() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), suite("greet {{ name }}"));
    bin()
        .args(["run", "--no-cache"])
        .current_dir(dir.path())
        .assert()
        .success();

    let run = latest_run(dir.path());
    let case = &run.cases[0];
    assert!(
        case.prompt_digest
            .as_deref()
            .is_some_and(|d| d.starts_with("blake3:")),
        "prompt_digest missing under --no-cache: {:?}",
        case.prompt_digest
    );
    assert!(
        case.provider_digest
            .as_deref()
            .is_some_and(|d| d.starts_with("blake3:")),
        "provider_digest missing under --no-cache: {:?}",
        case.provider_digest
    );
    assert!(
        case.assert_digest
            .as_deref()
            .is_some_and(|d| d.starts_with("blake3:")),
        "assert_digest missing: {:?}",
        case.assert_digest
    );
}

/// A prompt edit changes what the case asked, but not which model it asked —
/// the separation the single cache-key hash could not express.
#[test]
fn a_prompt_edit_moves_the_case_prompt_digest_but_not_its_provider_digest() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), suite("greet {{ name }}"));
    bin()
        .args(["run", "--no-cache"])
        .current_dir(dir.path())
        .assert()
        .success();
    let before = latest_run(dir.path()).cases[0].clone();

    write(dir.path(), suite("say hi to {{ name }}"));
    bin()
        .args(["run", "--no-cache"])
        .current_dir(dir.path())
        .assert()
        .success();
    let after = latest_run(dir.path()).cases[0].clone();

    assert_eq!(before.case_key, after.case_key, "same cell");
    assert_ne!(before.prompt_digest, after.prompt_digest);
    assert_eq!(before.provider_digest, after.provider_digest);
    assert_eq!(before.assert_digest, after.assert_digest);
}

/// Filters narrow which cells run; they do not change the suite. A digest that
/// moved under `--tag` would make every filtered CI job read as "the tests
/// changed".
#[test]
fn a_filtered_run_digests_the_whole_suite_not_the_subset() {
    let dir = tempfile::tempdir().unwrap();
    let body = suite("greet {{ name }}").replace(
        "  - id: t1\n    vars: {name: world}",
        "  - id: t1\n    tags: [smoke]\n    vars: {name: world}",
    );
    write(dir.path(), body.clone());
    bin().arg("run").current_dir(dir.path()).assert().success();
    let full = latest_run(dir.path()).digests.unwrap();

    bin()
        .args(["run", "--tag", "smoke", "--no-cache"])
        .current_dir(dir.path())
        .assert()
        .success();
    let filtered = latest_run(dir.path()).digests.unwrap();

    assert_eq!(full.tests, filtered.tests);
    assert_eq!(full.asserts, filtered.asserts);
}
