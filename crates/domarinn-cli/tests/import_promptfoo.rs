//! E2E: `domarinn import promptfoo` output round-trips through the real loader.
//!
//! The unit tests in `src/import_tests.rs` pin each mapping; this one proves the
//! mappings add up to a suite `domarinn validate` accepts cleanly — no errors
//! and no warnings — for a promptfoo config that exercises all of them.

mod common;

use std::path::{Path, PathBuf};

use common::bin;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/import_promptfoo")
}

/// Convert the fixture into a scratch dir (with the CSV it references beside
/// it), returning the dir and the converter's stdout.
fn convert_fixture() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    for name in ["promptfooconfig.yaml", "tests.csv"] {
        std::fs::copy(fixture_dir().join(name), dir.path().join(name)).unwrap();
    }
    let out = bin()
        .args(["import", "promptfoo", "promptfooconfig.yaml"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8(out.stdout).unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), &printed).unwrap();
    (dir, printed)
}

#[test]
fn representative_config_validates_with_no_errors_or_warnings() {
    let (dir, printed) = convert_fixture();
    let out = bin()
        .args(["validate", "domarinn.yaml"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "converted suite failed validate:\n{stdout}{stderr}\n--- suite ---\n{printed}"
    );
    for level in ["error:", "warning:"] {
        assert!(
            !stderr.contains(level) && !stdout.contains(level),
            "validate reported a {level}\n{stdout}{stderr}\n--- suite ---\n{printed}"
        );
    }

    // Every case resolves, including the matrix fan-out and the CSV source.
    let out = bin()
        .args(["list", "tests", "domarinn.yaml"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let listed = String::from_utf8_lossy(&out.stdout);
    assert_eq!(listed.lines().count(), 4, "{listed}");
}

/// The editor hint points at the published schema, not a local file the
/// converter cannot know exists.
#[test]
fn header_names_the_published_schema() {
    let (_dir, printed) = convert_fixture();
    let expected = format!(
        "# yaml-language-server: $schema={}",
        domarinn_core::CONFIG_SCHEMA_ID
    );
    assert!(printed.lines().any(|l| l == expected), "{printed}");
}

/// The only note this config earns is the one for the CSV source, which is
/// carried across as-is rather than converted.
#[test]
fn representative_config_needs_only_the_file_source_note() {
    let (_dir, printed) = convert_fixture();
    let notes: Vec<&str> = printed
        .lines()
        .filter(|l| l.starts_with("# NOTE:"))
        .collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("file://tests.csv"), "{notes:?}");
}
