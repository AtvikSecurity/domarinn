//! Sandboxed file-content vars: `!file "path"` / `{$file: "path"}`.
//!
//! A var can pull its value from a file next to the suite instead of writing it
//! inline — handy for large documents, golden fixtures, or reusable inputs:
//!
//! ```yaml
//! vars:
//!   document: !file "fixtures/article.txt"          # loaded as text
//!   schema:   { $file: "fixtures/schema.json" }     # parsed by extension → JSON
//!   payload:  { $file: "fixtures/probe.txt", raw: true }  # never templated
//! ```
//!
//! Resolution is deliberately staged:
//!
//! * The path is resolved through [`crate::sandbox`] relative to the suite
//!   directory — a fixture can never read outside it.
//! * The value is parsed by extension: `.json` → JSON, `.yaml`/`.yml` → YAML,
//!   anything else → text. `parse: false` forces text even for `.json`/`.yaml`.
//! * `raw: true` marks the loaded content [`Val::Raw`], so an untrusted fixture
//!   is passed to the provider verbatim and never runs through the template
//!   engine (the same SSTI guard `!raw` gives inline values).
//!
//! [`resolve_file_vars`] runs during [`crate::resolve::expand_tests`], **before**
//! any rendering, so by the time the runner renders a prompt every `{$file: …}`
//! marker has already become concrete content.
//!
//! Cache note: a file var's content enters the rendered vars, which are the
//! provider request identity (cache key) — so editing a fixture busts the cache
//! for cases that read it, exactly as an inline change would.

use std::path::Path;

use serde_json::Value as Json;

use crate::config::TestCase;
use crate::resolve::ResolveError;
use crate::sandbox;
use crate::val::{Val, FILE_KEY};

/// Resolve every `{$file: …}` var in `tests` to its file content, relative to
/// `base_dir`. A var whose value is not a file reference is left untouched.
pub fn resolve_file_vars(tests: &mut [TestCase], base_dir: &Path) -> Result<(), ResolveError> {
    for tc in tests.iter_mut() {
        for val in tc.vars.values_mut() {
            if let Some(spec) = FileSpec::from_json(val.as_json())? {
                *val = load_file_var(&spec, base_dir)?;
            }
        }
    }
    Ok(())
}

/// The parsed options of a `{$file: …}` reference.
struct FileSpec {
    path: String,
    /// Force text even for `.json`/`.yaml` extensions.
    parse: bool,
    /// Mark the loaded content raw (never rendered).
    raw: bool,
}

impl FileSpec {
    /// Recognize `{$file: "path", parse?, raw?}`. Returns `Ok(None)` when the
    /// value is not a file reference, and an error for a malformed one (a
    /// non-string `$file`, or an unknown sibling key — a likely typo).
    fn from_json(value: &Json) -> Result<Option<FileSpec>, ResolveError> {
        let Json::Object(map) = value else {
            return Ok(None);
        };
        let Some(path_val) = map.get(FILE_KEY) else {
            return Ok(None);
        };
        let path = path_val.as_str().ok_or_else(|| ResolveError::Parse {
            path: FILE_KEY.to_string(),
            message: "`$file` must be a string path".to_string(),
        })?;
        let mut spec = FileSpec {
            path: path.to_string(),
            parse: true,
            raw: false,
        };
        for (key, v) in map {
            match key.as_str() {
                FILE_KEY => {}
                "parse" => spec.parse = v.as_bool().ok_or_else(|| bad_option("parse", path))?,
                "raw" => spec.raw = v.as_bool().ok_or_else(|| bad_option("raw", path))?,
                other => {
                    return Err(ResolveError::Parse {
                        path: path.to_string(),
                        message: format!(
                            "unknown `$file` option '{other}' (expected only 'parse' / 'raw')"
                        ),
                    })
                }
            }
        }
        Ok(Some(spec))
    }
}

fn bad_option(name: &str, path: &str) -> ResolveError {
    ResolveError::Parse {
        path: path.to_string(),
        message: format!("`$file` option '{name}' must be a boolean"),
    }
}

/// Read and (optionally) parse one file reference into a [`Val`].
fn load_file_var(spec: &FileSpec, base_dir: &Path) -> Result<Val, ResolveError> {
    let resolved = sandbox::resolve_within(base_dir, &spec.path)?;
    let text = std::fs::read_to_string(&resolved).map_err(|source| ResolveError::Io {
        path: resolved.display().to_string(),
        source,
    })?;

    let ext = resolved
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let content = if spec.parse {
        match ext.as_str() {
            "json" => serde_json::from_str::<Json>(&text).map_err(|e| ResolveError::Parse {
                path: resolved.display().to_string(),
                message: e.to_string(),
            })?,
            "yaml" | "yml" => {
                serde_yaml_ng::from_str::<Json>(&text).map_err(|e| ResolveError::Parse {
                    path: resolved.display().to_string(),
                    message: e.to_string(),
                })?
            }
            _ => Json::String(text),
        }
    } else {
        Json::String(text)
    };

    Ok(if spec.raw {
        Val::Raw(content)
    } else {
        Val::Tpl(content)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn case_with_var(name: &str, value: Json) -> TestCase {
        let mut vars: BTreeMap<String, Val> = BTreeMap::new();
        vars.insert(name.to_string(), Val::classify(value));
        TestCase {
            vars,
            ..Default::default()
        }
    }

    #[test]
    fn loads_text_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("doc.txt"), "hello from disk").unwrap();
        let mut cases = vec![case_with_var(
            "document",
            serde_json::json!({ "$file": "doc.txt" }),
        )];
        resolve_file_vars(&mut cases, dir.path()).unwrap();
        assert_eq!(
            cases[0].vars["document"],
            Val::Tpl(Json::String("hello from disk".into()))
        );
    }

    #[test]
    fn parses_json_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cfg.json"), r#"{"k": [1, 2, 3]}"#).unwrap();
        let mut cases = vec![case_with_var(
            "schema",
            serde_json::json!({ "$file": "cfg.json" }),
        )];
        resolve_file_vars(&mut cases, dir.path()).unwrap();
        assert_eq!(
            cases[0].vars["schema"],
            Val::Tpl(serde_json::json!({ "k": [1, 2, 3] }))
        );
    }

    #[test]
    fn parses_yaml_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cfg.yaml"), "k:\n  - a\n  - b\n").unwrap();
        let mut cases = vec![case_with_var(
            "data",
            serde_json::json!({ "$file": "cfg.yaml" }),
        )];
        resolve_file_vars(&mut cases, dir.path()).unwrap();
        assert_eq!(
            cases[0].vars["data"],
            Val::Tpl(serde_json::json!({ "k": ["a", "b"] }))
        );
    }

    #[test]
    fn parse_false_forces_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cfg.json"), r#"{"k": 1}"#).unwrap();
        let mut cases = vec![case_with_var(
            "literal",
            serde_json::json!({ "$file": "cfg.json", "parse": false }),
        )];
        resolve_file_vars(&mut cases, dir.path()).unwrap();
        assert_eq!(
            cases[0].vars["literal"],
            Val::Tpl(Json::String(r#"{"k": 1}"#.into()))
        );
    }

    #[test]
    fn raw_true_marks_content_raw() {
        // An untrusted fixture holding an SSTI payload must never be templated.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("probe.txt"), "{{7*7}}").unwrap();
        let mut cases = vec![case_with_var(
            "payload",
            serde_json::json!({ "$file": "probe.txt", "raw": true }),
        )];
        resolve_file_vars(&mut cases, dir.path()).unwrap();
        assert_eq!(
            cases[0].vars["payload"],
            Val::Raw(Json::String("{{7*7}}".into()))
        );
        assert!(cases[0].vars["payload"].is_raw());
    }

    #[test]
    fn escape_is_rejected() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("secret.txt"), "TOP SECRET").unwrap();
        let base = parent.path().join("suite");
        std::fs::create_dir(&base).unwrap();
        let mut cases = vec![case_with_var(
            "leak",
            serde_json::json!({ "$file": "../secret.txt" }),
        )];
        let err = resolve_file_vars(&mut cases, &base).unwrap_err();
        assert!(
            err.to_string().contains("refuses to read outside"),
            "sandbox escape must be rejected: {err}"
        );
    }

    #[test]
    fn missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut cases = vec![case_with_var(
            "gone",
            serde_json::json!({ "$file": "nope.txt" }),
        )];
        let err = resolve_file_vars(&mut cases, dir.path()).unwrap_err();
        assert!(matches!(err, ResolveError::Io { .. }), "{err:?}");
    }

    #[test]
    fn unknown_option_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("doc.txt"), "x").unwrap();
        let mut cases = vec![case_with_var(
            "bad",
            serde_json::json!({ "$file": "doc.txt", "prase": false }),
        )];
        let err = resolve_file_vars(&mut cases, dir.path()).unwrap_err();
        assert!(matches!(err, ResolveError::Parse { .. }), "{err:?}");
        assert!(err.to_string().contains("prase"));
    }

    #[test]
    fn non_file_var_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let mut cases = vec![case_with_var("plain", serde_json::json!("just a value"))];
        resolve_file_vars(&mut cases, dir.path()).unwrap();
        assert_eq!(
            cases[0].vars["plain"],
            Val::Tpl(Json::String("just a value".into()))
        );
    }
}
