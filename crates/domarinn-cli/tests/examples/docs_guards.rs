//! The guards that tie `examples/` to the documentation.
//!
//! Everything here is a *structural* claim about the ladder: every shipped
//! directory has a table row, every example is transcluded on a page, and the
//! index table maps every number to the page that really shows it. The
//! behavioral claims — what each example does when run — live in the table
//! rows and `examples.rs` itself.

use std::collections::BTreeSet;
use std::path::Path;

use crate::table::EXAMPLES;
use crate::{examples_root, repo_root};

/// `NN-kebab-name`: two digits, a dash, then lowercase words.
///
/// Policed rather than merely conventional because the docs address examples by
/// this shape, and a misnamed directory is one the ladder cannot transclude.
fn is_ladder_name(name: &str) -> bool {
    let Some((num, rest)) = name.split_once('-') else {
        return false;
    };
    num.len() == 2
        && num.chars().all(|c| c.is_ascii_digit())
        && !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Every example directory that ships, by name.
fn shipped_example_dirs() -> BTreeSet<String> {
    let root = examples_root();
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(&root).expect("examples/ exists") {
        let entry = entry.expect("readable directory entry");
        if !entry.file_type().expect("file type").is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            is_ladder_name(&name),
            "examples/{name}/ does not match the NN-kebab-name scheme the docs \
             address examples by. Rename it, or move it out of examples/ if it \
             is not an example."
        );
        assert!(
            entry.path().join("domarinn.yaml").is_file(),
            "examples/{name}/ has no domarinn.yaml, so there is no suite to run"
        );
        out.insert(name);
    }
    out
}

/// A directory under `examples/` that no row covers is an example nobody runs,
/// and an example nobody runs is a page that lies. This is the guard that makes
/// "every example is verified end to end" a property of the repository rather
/// than of whoever reviewed the last pull request.
#[test]
fn every_shipped_example_is_in_the_table() {
    let shipped = shipped_example_dirs();
    // Guard against a vacuous pass: a broken glob — a moved crate, a renamed
    // directory — yields an empty set that satisfies every assertion below,
    // and this test would stay green forever while covering nothing.
    assert!(
        shipped.len() >= 4,
        "found {} example directories under {}; the glob is broken, not the repo",
        shipped.len(),
        examples_root().display()
    );

    let covered: BTreeSet<&str> = EXAMPLES.iter().map(|e| e.dir).collect();
    assert_eq!(
        covered.len(),
        EXAMPLES.len(),
        "two rows name the same example directory"
    );

    let untested: Vec<&str> = shipped
        .iter()
        .filter(|d| !covered.contains(d.as_str()))
        .map(String::as_str)
        .collect();
    assert!(
        untested.is_empty(),
        "these examples ship but nothing runs them: {untested:?}\n\
         Add a row to crates/domarinn-cli/tests/examples/table.rs — a directory \
         under examples/ and a row in that table are the same thing."
    );

    let stale: Vec<&str> = EXAMPLES
        .iter()
        .map(|e| e.dir)
        .filter(|d| !shipped.contains(*d))
        .collect();
    assert!(
        stale.is_empty(),
        "these rows name directories that no longer exist: {stale:?}"
    );
}

/// An example the documentation never shows is dead weight that still has to be
/// kept green; a `--8<--` pointing at a file that is not there renders as a
/// broken page.
///
/// MkDocs' `check_paths` catches only the second, and only when the site is
/// built. The reverse — an example no page transcludes — is invisible to it,
/// which is exactly the direction that rots: an example gets renamed, the page
/// keeps showing the old one, and nobody notices because both still exist.
#[test]
fn every_example_is_transcluded_and_every_transclusion_resolves() {
    let root = repo_root();
    let includes = snippet_includes(&root.join("docs"));
    assert!(
        !includes.is_empty(),
        "no `--8<--` includes found under docs/; the scanner is broken, or the \
         examples are not being transcluded anywhere"
    );

    // Forward: a snippet path must exist. Resolved against the repo root, which
    // is what pymdownx.snippets' `base_path` is set to in mkdocs.yml.
    let dangling: Vec<&(String, String)> = includes
        .iter()
        .filter(|(_, path)| !root.join(path).is_file())
        .collect();
    assert!(
        dangling.is_empty(),
        "doc snippets point at files that do not exist: {dangling:#?}"
    );

    // Reverse: every example is shown somewhere.
    let shown: BTreeSet<&str> = includes
        .iter()
        .filter_map(|(_, path)| path.strip_prefix("examples/"))
        .filter_map(|rest| rest.split('/').next())
        .collect();
    let undocumented: Vec<&str> = EXAMPLES
        .iter()
        .map(|e| e.dir)
        .filter(|d| !shown.contains(d))
        .collect();
    assert!(
        undocumented.is_empty(),
        "these examples are tested but no page shows them: {undocumented:?}\n\
         Transclude each with `--8<-- \"examples/<dir>/domarinn.yaml\"`, or \
         delete it: an example whose only reader is CI is a maintenance cost \
         with no payer."
    );
}

/// The examples index (docs/examples/index.md) is the one table mapping
/// example numbers to group pages — the docs' front door. It is hand-written,
/// and the transclusion guard above cannot see it (it scans `--8<--`, and the
/// index transcludes nothing). Guard it directly: exactly one numbered row per
/// shipped example, each linking to an anchor on the page that really
/// transcludes that example. Without this, example 33 could land fully
/// transcluded with every other guard green and still be invisible from the
/// front door.
#[test]
fn the_examples_index_table_maps_every_example_to_its_page() {
    let root = repo_root();
    let index = std::fs::read_to_string(root.join("docs/examples/index.md"))
        .expect("docs/examples/index.md exists");
    let includes = snippet_includes(&root.join("docs/examples"));

    let mut unlinked = Vec::new();
    for spec in EXAMPLES {
        let num = &spec.dir[..2];
        // The group pages that transclude this example, by file name.
        let mut pages: Vec<String> = includes
            .iter()
            .filter(|(_, path)| {
                path.strip_prefix("examples/")
                    .and_then(|r| r.split('/').next())
                    == Some(spec.dir)
            })
            .filter_map(|(page, _)| {
                Path::new(page)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
            .collect();
        pages.sort();
        pages.dedup();
        // A row link looks like `(first-steps.md#example-01--hello-eval)`.
        let linked = pages
            .iter()
            .any(|page| index.contains(&format!("({page}#example-{num}--")));
        if !linked {
            unlinked.push((spec.dir, pages));
        }
    }
    assert!(
        unlinked.is_empty(),
        "these examples have no index-table row linking the page that \
         transcludes them: {unlinked:?}\n\
         Add a `| NN | [title](<page>.md#example-NN--…) | …|` row to \
         docs/examples/index.md (empty page list = nothing under \
         docs/examples/ transcludes the example at all)."
    );

    // No phantom rows either: every numbered row must correspond to a shipped
    // example, so the count matches exactly.
    let rows = index
        .lines()
        .filter(|l| {
            let b = l.as_bytes();
            b.starts_with(b"| ") && b.len() > 4 && b[2].is_ascii_digit() && b[3].is_ascii_digit()
        })
        .count();
    assert_eq!(
        rows,
        EXAMPLES.len(),
        "docs/examples/index.md has {} numbered rows for {} shipped examples — \
         a row was added or removed without the other side",
        rows,
        EXAMPLES.len()
    );
}

/// Every `--8<-- "path"` in every markdown file under `dir`, as (page, path).
fn snippet_includes(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).expect("readable docs directory") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            for line in text.lines() {
                let Some(rest) = line.trim_start().strip_prefix("--8<--") else {
                    continue;
                };
                let quoted = rest.trim();
                let include = quoted
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .unwrap_or_else(|| {
                        panic!(
                            "{}: `--8<--` is not the inline quoted form this repo \
                             uses, so nothing checks it: {line}",
                            path.display()
                        )
                    });
                // A `file:section` or `file:5:8` include names the file before
                // the first colon.
                let file = include.split(':').next().unwrap_or(include);
                out.push((path.display().to_string(), file.to_string()));
            }
        }
    }
    out
}
