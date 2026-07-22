//! Path sandboxing for `file://` references.
//!
//! Every on-disk read a suite triggers — prompt/message content, file-var
//! fixtures, `file://` test-file globs — is resolved **relative to the suite
//! directory** and must stay inside it. Without a guard, `file://../../etc/passwd`
//! (or a symlink pointing outside) would let an attacker-supplied suite read
//! arbitrary host files. [`resolve_within`] closes that hole.
//!
//! The guard is two-layered, so it holds even for paths that do not yet exist:
//!
//! 1. A **structural** check rejects absolute paths and any `..` (parent-dir),
//!    root, or drive-prefix component before touching the filesystem. This alone
//!    stops `../secret`, and it works on plain paths and glob patterns alike
//!    (glob metacharacters like `*`/`**`/`?` are ordinary path components).
//! 2. A **canonicalization** check resolves the target (or, if it does not exist
//!    yet, its nearest existing ancestor) and verifies it still lives under the
//!    canonical suite directory — catching a symlink inside the suite that points
//!    out of it, which a component check alone would miss.

use std::path::{Component, Path, PathBuf};

/// A `file://` reference that would read outside the suite directory.
#[derive(Debug, thiserror::Error)]
#[error("'{spec}' refuses to read outside the suite directory (base: {base})")]
pub struct SandboxError {
    /// The offending reference (as written, or the resolved match for a glob).
    pub spec: String,
    /// The suite directory the reference had to stay within.
    pub base: String,
}

impl SandboxError {
    fn new(base: &Path, spec: impl Into<String>) -> SandboxError {
        SandboxError {
            spec: spec.into(),
            base: base.display().to_string(),
        }
    }
}

/// Resolve a single `file://` relative path to a concrete path inside
/// `base_dir`, rejecting any escape. The returned path is `base_dir` joined with
/// `rel`; it may or may not exist (the caller reads it and handles I/O errors).
pub fn resolve_within(base_dir: &Path, rel: &str) -> Result<PathBuf, SandboxError> {
    reject_bad_spec(base_dir, rel)?;
    let joined = base_dir.join(rel);
    assert_within(base_dir, &joined, rel)?;
    Ok(joined)
}

/// Structural guard on a `file://` spec: reject absolute paths and any `..`,
/// root, or drive-prefix component. Runs before any filesystem access, so a
/// traversal into a not-yet-existing path is still caught.
pub fn reject_bad_spec(base_dir: &Path, rel: &str) -> Result<(), SandboxError> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(SandboxError::new(base_dir, rel));
    }
    for comp in rel_path.components() {
        match comp {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SandboxError::new(base_dir, rel));
            }
            Component::Normal(_) | Component::CurDir => {}
        }
    }
    Ok(())
}

/// Verify an already-resolved `path` canonicalizes to somewhere inside
/// `base_dir` (the symlink-escape defense). `spec` labels the error. A path that
/// does not exist yet is checked via its nearest existing ancestor.
pub fn assert_within(base_dir: &Path, path: &Path, spec: &str) -> Result<(), SandboxError> {
    let canonical_base = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.to_path_buf());
    if let Some(canonical) = canonicalize_existing(path) {
        if !canonical.starts_with(&canonical_base) {
            return Err(SandboxError::new(base_dir, spec));
        }
    }
    Ok(())
}

/// Canonicalize `path` if it exists; otherwise canonicalize its nearest existing
/// ancestor and re-append the remaining (not-yet-existing) tail, so a symlinked
/// ancestor pointing outside `base` is still resolved and caught. Returns `None`
/// only when no ancestor (not even the root) can be canonicalized.
fn canonicalize_existing(path: &Path) -> Option<PathBuf> {
    if let Ok(canon) = path.canonicalize() {
        return Some(canon);
    }
    let mut ancestors = path.ancestors();
    let _self = ancestors.next(); // skip the path itself — canonicalize already failed
    for ancestor in ancestors {
        if let Ok(canon) = ancestor.canonicalize() {
            return match path.strip_prefix(ancestor) {
                Ok(tail) => Some(canon.join(tail)),
                Err(_) => Some(canon),
            };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_relative_path_resolves_within() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve_within(dir.path(), "sub/doc.txt").unwrap();
        assert!(resolved.starts_with(dir.path()));
        assert!(resolved.ends_with("sub/doc.txt"));
    }

    #[test]
    fn resolves_nonexistent_path_within() {
        // A not-yet-existing target is allowed as long as it stays inside base.
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_within(dir.path(), "not/created/yet.txt").is_ok());
    }

    #[test]
    fn parent_traversal_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_within(dir.path(), "../secret.txt").unwrap_err();
        assert!(err.to_string().contains("refuses to read outside"));
    }

    #[test]
    fn deep_parent_traversal_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_within(dir.path(), "a/b/../../../etc/passwd").is_err());
    }

    #[test]
    fn absolute_path_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_within(dir.path(), "/etc/passwd").is_err());
    }

    #[test]
    fn parent_traversal_to_a_real_file_is_still_rejected() {
        // The escape target actually exists — the guard must reject on the `..`
        // structure regardless, before ever reading it.
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("secret.txt"), "TOP SECRET").unwrap();
        let base = parent.path().join("suite");
        std::fs::create_dir(&base).unwrap();
        assert!(resolve_within(&base, "../secret.txt").is_err());
    }

    #[test]
    fn symlink_escape_is_rejected() {
        // A symlink inside the suite pointing outside it must not be a read hole.
        // (Structural check passes — no `..` — so the canonicalization layer is
        // what catches this.)
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "TOP SECRET").unwrap();
        let base = tempfile::tempdir().unwrap();
        let link = base.path().join("escape");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            let err = resolve_within(base.path(), "escape/secret.txt").unwrap_err();
            assert!(err.to_string().contains("refuses to read outside"));
        }
        #[cfg(not(unix))]
        let _ = link;
    }

    #[test]
    fn nested_paths_within_are_allowed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/c.txt"), "ok").unwrap();
        let resolved = resolve_within(dir.path(), "a/b/c.txt").unwrap();
        assert_eq!(std::fs::read_to_string(resolved).unwrap(), "ok");
    }
}
