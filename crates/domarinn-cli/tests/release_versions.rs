//! Every version this repo publishes is one version, and release-please writes
//! all of them.
//!
//! # Why this guard exists
//!
//! The Claude Code plugin manifests carry a version, and they were added
//! without being registered in `release-please-config.json`. Nothing noticed:
//! the repo shipped 0.4.0 and then 0.5.0 while the plugin and its marketplace
//! entry sat at 0.3.1, telling anyone who installed it they were getting a
//! release that was two behind. There was no failure to see — a version file
//! release-please does not know about simply never changes.
//!
//! So the drift is checked from both ends. [`every_published_version_matches`]
//! catches a file that has fallen behind, and
//! [`every_version_file_is_registered_with_release_please`] catches the cause:
//! a version file that nothing bumps. The second is the one that matters —
//! without it, adding a new manifest re-creates exactly the situation above,
//! and the first test would only start failing one release later.
//!
//! `Cargo.toml` is the source rather than a member of the set: it is what
//! `.release-please-manifest.json` tracks, and `CARGO_PKG_VERSION` is what the
//! binary and the MCP endpoint's `serverInfo` report.

use std::path::{Path, PathBuf};

/// Files that carry the repo's version, and the release-please `jsonpath` that
/// writes each one. Add a row here when a new manifest appears — the second
/// test then requires it to be registered before it can pass.
const VERSIONED_JSON: &[(&str, &[&str])] = &[
    ("web/package.json", &["$.version"]),
    ("plugin/.claude-plugin/plugin.json", &["$.version"]),
    // Both `metadata.version` and every `plugins[].version`. One recursive
    // path rather than one entry per field: in a marketplace manifest every
    // `version` is the plugin's version, and a single expression cannot
    // half-apply the way two entries for one file could.
    (".claude-plugin/marketplace.json", &["$..version"]),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves from crates/domarinn-cli")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {relative}: {e}"))
}

/// The workspace version, from the one place release-please's manifest tracks.
///
/// Scanned rather than parsed with a TOML crate: this needs exactly one value
/// out of a file whose shape is fixed, and a dependency added for a guard is a
/// dependency the release build carries.
fn workspace_version() -> String {
    let cargo = read("Cargo.toml");
    let section = cargo
        .split_once("[workspace.package]")
        .expect("Cargo.toml has a [workspace.package] section")
        .1;
    for line in section.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        if let Some(rest) = line.strip_prefix("version") {
            if let Some(value) = rest.split('"').nth(1) {
                return value.to_string();
            }
        }
    }
    panic!("no `version = \"…\"` under [workspace.package] in Cargo.toml");
}

/// Every `version` string anywhere in a JSON document, with its location.
fn versions_in(value: &serde_json::Value, at: &str, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let path = format!("{at}.{key}");
                if key == "version" {
                    if let Some(v) = child.as_str() {
                        out.push((path.clone(), v.to_string()));
                    }
                }
                versions_in(child, &path, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                versions_in(child, &format!("{at}[{i}]"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn every_published_version_matches_the_workspace() {
    let expected = workspace_version();
    let mut wrong = Vec::new();

    for (file, _) in VERSIONED_JSON {
        let doc: serde_json::Value =
            serde_json::from_str(&read(file)).unwrap_or_else(|e| panic!("{file} is not JSON: {e}"));
        let mut found = Vec::new();
        versions_in(&doc, "$", &mut found);
        assert!(
            !found.is_empty(),
            "{file} is listed as carrying a version but has no `version` field. \
             Remove the row from VERSIONED_JSON, or add the field back."
        );
        for (path, actual) in found {
            if actual != expected {
                wrong.push(format!("  {file} {path} = {actual}"));
            }
        }
    }

    assert!(
        wrong.is_empty(),
        "these published versions disagree with the workspace version \
         ({expected}):\n{}\n\nRelease-please writes all of them, so this \
         normally means a file was edited by hand or added without being \
         registered. See release-please-config.json.",
        wrong.join("\n")
    );
}

/// The test that prevents a recurrence rather than reporting one.
///
/// A version file nothing bumps produces no failure at all until a release has
/// already shipped past it, which is exactly how the plugin reached 0.3.1 while
/// the repo was at 0.5.0.
#[test]
fn every_version_file_is_registered_with_release_please() {
    let config: serde_json::Value = serde_json::from_str(&read("release-please-config.json"))
        .expect("release-please-config.json is JSON");
    let extra = config["packages"]["."]["extra-files"]
        .as_array()
        .expect("the root package declares extra-files");

    for (file, paths) in VERSIONED_JSON {
        for wanted in *paths {
            let registered = extra.iter().any(|entry| {
                entry["path"].as_str() == Some(file) && entry["jsonpath"].as_str() == Some(wanted)
            });
            assert!(
                registered,
                "{file} carries the repo's version but release-please does not \
                 write it. Add to release-please-config.json extra-files:\n  \
                 {{ \"type\": \"json\", \"path\": \"{file}\", \"jsonpath\": \"{wanted}\" }}"
            );
        }
    }
}

/// The manifest is what release-please reads to decide the next version, so a
/// workspace that has drifted from it would bump from the wrong base.
#[test]
fn the_release_manifest_agrees_with_the_workspace() {
    let manifest: serde_json::Value = serde_json::from_str(&read(".release-please-manifest.json"))
        .expect(".release-please-manifest.json is JSON");
    assert_eq!(
        manifest["."].as_str(),
        Some(workspace_version().as_str()),
        "Cargo.toml's workspace version and .release-please-manifest.json disagree"
    );
}
