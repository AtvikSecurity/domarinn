//! Ratchet: no source file may exceed 1000 lines.
//!
//! Large files signal a module doing too much; split them. If a file genuinely
//! must exceed the cap (e.g. a generated artifact), add its repo-relative path
//! to `EXCEPTIONS` with a comment explaining why.

use std::path::{Path, PathBuf};

/// Hard cap on lines per source file.
const MAX_LINES: usize = 1000;

/// Source extensions this ratchet governs.
const SOURCE_EXTS: &[&str] = &["rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "css", "scss"];

/// Directories never descended into.
///
/// These are dependency and build trees, not source. `.venv` is the one that is
/// easy to miss: `uv run` (which builds the docs site) materializes the pinned
/// MkDocs toolchain into `.venv/` at the repo root, and Material ships vendored
/// JavaScript far past the cap — so without this entry, building the docs makes
/// the ratchet fail on somebody else's code.
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    ".git",
    ".domarinn",
    ".worktrees",
    // Agent scratch, and — the reason this is here — `.claude/worktrees/`,
    // where tooling checks the repo out at other revisions. The whole
    // directory is gitignored, so nothing under it is code this checkout is
    // responsible for. Worse than merely noisy: `EXCEPTIONS` is matched on
    // repo-relative paths, so an excepted file reappears under a worktree
    // prefix that matches nothing and fails the ratchet on a file that is
    // *already* allowed to be long. Anyone with a stale worktree then has a
    // permanently red `mise run ci`, which is how a ratchet stops being read.
    ".claude",
    ".venv",
    "site",
];

/// Repo-relative paths permitted to exceed the cap (with justification).
const EXCEPTIONS: &[&str] = &[
    // One ingest transaction: the prepared row structs and the INSERT that
    // writes them must stay adjacent, and every promoted column adds a line to
    // each. Splitting it would separate a column's field, its population and
    // its bind across files without making any of them easier to read.
    "crates/domarinn-server/src/storage/runs.rs",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("resolve repo root")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            walk(&path, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if SOURCE_EXTS.contains(&ext) {
                out.push(path);
            }
        }
    }
}

#[test]
fn no_source_file_exceeds_line_cap() {
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(
        !files.is_empty(),
        "walk found no source files under {root:?}"
    );

    let mut offenders: Vec<(String, usize)> = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if EXCEPTIONS.contains(&rel.as_str()) {
            continue;
        }
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue, // skip non-UTF8/binary
        };
        let lines = content.lines().count();
        if lines > MAX_LINES {
            offenders.push((rel, lines));
        }
    }

    assert!(
        offenders.is_empty(),
        "these source files exceed {MAX_LINES} lines (split them, or add to EXCEPTIONS): {:#?}",
        offenders
    );
}
