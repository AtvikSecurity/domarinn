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

use crate::config::{AssertKind, TestCase};
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

/// Resolve `{$file: …}` inside every assertion's `Val`-typed field, the same
/// way [`resolve_file_vars`] does for test vars.
///
/// This closes a second silent pass hiding behind the first. `resolve_file_vars`
/// walks only `tc.vars`, so `schema: {$file: "s.json"}` reached the assertion as
/// the literal object `{"$file": "s.json"}` — which, read as a JSON Schema, is a
/// document of entirely unknown keywords and therefore **matches everything**. A
/// user who moved their schema into a file got a green check that validated
/// nothing at all.
///
/// The same was true of `equals` and `similar`, which compared output against
/// the marker object rather than the file's contents, and of `tool-call`'s
/// `args`/`schema`. All are handled here.
///
/// **Every `Val`-typed assertion field belongs in the match below.** One left
/// out is not a missing feature, it is a silent pass: `tool-call` was added with
/// a `schema:` and reintroduced the exact green-check-that-validated-nothing
/// this function was written to remove.
///
/// Runs inside `expand_tests`, so a schema is concrete content long before the
/// runner evaluates anything, and inherits the same sandbox as every other
/// `$file` reference.
pub fn resolve_assert_file_vals(
    tests: &mut [TestCase],
    base_dir: &Path,
) -> Result<(), ResolveError> {
    for tc in tests.iter_mut() {
        for assert in tc.assert.iter_mut() {
            // A `Vec` rather than one slot: `tool-call` carries two `Val` fields,
            // and an assertion kind that grew a second one is exactly how the
            // gap this function closes reappeared.
            let vals: Vec<&mut crate::val::Val> = match &mut assert.kind {
                AssertKind::ContainsJson { schema } => schema.as_mut().into_iter().collect(),
                AssertKind::Equals { value } => vec![value],
                AssertKind::Similar { value, .. } => vec![value],
                AssertKind::ToolCall { args, schema, .. } => {
                    args.as_mut().into_iter().chain(schema.as_mut()).collect()
                }
                _ => Vec::new(),
            };
            for val in vals {
                if let Some(spec) = FileSpec::from_json(val.as_json())? {
                    *val = load_file_var(&spec, base_dir)?;
                }
            }
        }
    }
    Ok(())
}

/// The key marking a computed content digest.
const DIGEST_KEY: &str = "$digest";

/// Resolve `cache_salt: {$digest: "glob"}` into the digest of the matched
/// files' contents.
///
/// `cache_salt` is used verbatim and deliberately not templated, because a
/// useful salt is a digest of something domarinn cannot see: the system under
/// test's own prompt files, resolved across a process boundary. That left every
/// consumer computing the digest outside the suite — in practice, writing a
/// whole test *generator* whose only job was injecting one field.
///
/// This is the missing half. The glob is templated (so a case can digest the
/// specific file it exercises, e.g. `prompts/{{ prompt_id }}.md`), the matched
/// files are read in sorted order, and the digest of their contents becomes the
/// salt. Same sandbox as every other file reference — the salt is a suite-
/// authored path, and a suite must not be able to read outside its own tree.
///
/// Files are hashed with their relative paths interleaved, so moving content
/// between two matched files changes the salt. Hashing contents alone would
/// not, and "the same bytes in a different arrangement" is a real edit.
pub fn resolve_digest_salts(
    tests: &mut [TestCase],
    base_dir: &Path,
    engine: &crate::template::TemplateEngine,
) -> Result<(), ResolveError> {
    for tc in tests.iter_mut() {
        let Some(salt) = &tc.cache_salt else { continue };
        let Some(spec) = digest_spec(salt) else {
            continue;
        };
        let vars = serde_json::Value::Object(
            tc.vars
                .iter()
                .map(|(k, v)| (k.clone(), v.as_json().clone()))
                .collect(),
        );
        let pattern = engine
            .render_str(spec, &vars)
            .map_err(|e| ResolveError::Parse {
                path: DIGEST_KEY.to_string(),
                message: format!("rendering `$digest` glob `{spec}`: {e}"),
            })?;
        tc.cache_salt = Some(digest_of_glob(&pattern, base_dir)?);
    }
    Ok(())
}

/// The glob behind a `$digest:` salt, or `None` for an ordinary opaque salt.
///
/// One reader for both salt scopes, so a provider and a case can never disagree
/// about what counts as a digest spec.
fn digest_spec(salt: &str) -> Option<&str> {
    salt.strip_prefix("$digest:").map(str::trim)
}

/// Resolve a **provider's** `$digest:` salt, if it has one.
///
/// The provider-scope twin of [`resolve_digest_salts`]. Both scopes advertise
/// `$digest:` — [`crate::config::ProviderKind::Exec::cache_salt`] offers it as
/// the way to pin a provider to its sources, and the rebuild warning the runner
/// emits on a stale hit (`runner_cache`) tells you to reach for it by name — but
/// only the case scope ever resolved it. A provider salt went verbatim into the cache key, so
/// `"$digest: src/**/*.rs"` keyed every request on that literal 20-odd-character
/// string: a constant that never moves when the sources do. It failed silently
/// and looked like it worked, which is the worst way for a cache pin to be wrong.
///
/// Two deliberate differences from the case scope:
///
/// - **The glob is not templated.** A case renders it against its own `vars` so
///   each case can digest the one file it exercises; a provider has no vars, and
///   inventing a namespace for it would be a second, subtly different templating
///   context for no gain.
/// - **`base_dir` is required.** The case scope always has the suite directory;
///   a provider can be built without one (unit tests, embedders), and resolving a
///   relative glob against the process cwd would silently key on whatever
///   directory the caller happened to be standing in — the exact
///   machine-dependence 0.5.0 removed from these keys.
///
/// Returns the salt to actually use: the digest for a `$digest:` spec, the
/// original string otherwise.
pub fn resolve_provider_digest_salt(
    salt: Option<&str>,
    base_dir: Option<&Path>,
) -> Result<Option<String>, ResolveError> {
    let Some(salt) = salt else { return Ok(None) };
    let Some(spec) = digest_spec(salt) else {
        return Ok(Some(salt.to_string()));
    };
    let base_dir = base_dir.ok_or_else(|| ResolveError::Parse {
        path: DIGEST_KEY.to_string(),
        message: format!(
            "`$digest: {spec}` needs a suite directory to resolve against, but this \
             provider was built without one"
        ),
    })?;
    Ok(Some(digest_of_glob(spec, base_dir)?))
}

/// blake3 of every file matched by `pattern`, in sorted order.
fn digest_of_glob(pattern: &str, base_dir: &Path) -> Result<String, ResolveError> {
    sandbox::reject_bad_spec(base_dir, pattern)?;
    let joined = base_dir.join(pattern);
    let joined_str = joined.to_string_lossy().to_string();
    let matches = glob::glob(&joined_str).map_err(|source| ResolveError::Glob {
        pattern: joined_str.clone(),
        source,
    })?;

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for path in matches.flatten() {
        if path.is_file() {
            sandbox::assert_within(base_dir, &path, &path.to_string_lossy())?;
            files.push(path);
        }
    }
    files.sort();

    // A glob that matches nothing is a mistake, not an empty digest: it would
    // produce one constant salt shared by every such case, silently collapsing
    // the cache separation the salt exists to provide.
    if files.is_empty() {
        return Err(ResolveError::Parse {
            path: DIGEST_KEY.to_string(),
            message: format!("`$digest: {pattern}` matched no files"),
        });
    }

    let mut hasher = blake3::Hasher::new();
    for path in &files {
        let rel = path.strip_prefix(base_dir).unwrap_or(path);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        let bytes = std::fs::read(path).map_err(|source| ResolveError::Io {
            path: path.display().to_string(),
            source,
        })?;
        hasher.update(&bytes);
        hasher.update(&[0]);
    }
    Ok(format!("blake3:{}", hasher.finalize().to_hex()))
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

    /// Every `Val`-typed assertion field, or the omission is a silent pass: an
    /// unresolved `{"$file": …}` read as a JSON Schema is a document of entirely
    /// unknown keywords, which validates *everything*.
    #[test]
    fn every_val_typed_assert_field_resolves_its_file_reference() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("schema.json"),
            r#"{"type": "object", "required": ["city"]}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("args.json"), r#"{"city": "Oslo"}"#).unwrap();

        let file = |name: &str| Val::classify(serde_json::json!({"$file": name}));
        let mut cases = vec![TestCase {
            assert: vec![
                crate::config::Assert {
                    weight: 1.0,
                    negate: false,
                    kind: AssertKind::ToolCall {
                        name: "get_weather".into(),
                        args: Some(file("args.json")),
                        schema: Some(file("schema.json")),
                    },
                },
                crate::config::Assert {
                    weight: 1.0,
                    negate: false,
                    kind: AssertKind::ContainsJson {
                        schema: Some(file("schema.json")),
                    },
                },
                crate::config::Assert {
                    weight: 1.0,
                    negate: false,
                    kind: AssertKind::Equals {
                        value: file("args.json"),
                    },
                },
                crate::config::Assert {
                    weight: 1.0,
                    negate: false,
                    kind: AssertKind::Similar {
                        value: file("args.json"),
                        threshold: None,
                    },
                },
            ],
            ..Default::default()
        }];
        resolve_assert_file_vals(&mut cases, dir.path()).unwrap();

        for assert in &cases[0].assert {
            let rendered = serde_json::to_string(&assert.kind).unwrap();
            assert!(
                !rendered.contains("$file"),
                "an unresolved marker survived in {rendered}"
            );
        }
        let AssertKind::ToolCall { args, schema, .. } = &cases[0].assert[0].kind else {
            panic!("shape changed");
        };
        assert_eq!(
            schema.as_ref().unwrap().as_json(),
            &serde_json::json!({"type": "object", "required": ["city"]})
        );
        assert_eq!(
            args.as_ref().unwrap().as_json(),
            &serde_json::json!({"city": "Oslo"})
        );
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

#[cfg(test)]
mod digest_salt_tests {
    use super::*;
    use crate::template::TemplateEngine;

    fn case(salt: &str, vars: &[(&str, &str)]) -> TestCase {
        let mut tc = TestCase {
            cache_salt: Some(salt.to_string()),
            ..Default::default()
        };
        for (k, v) in vars {
            tc.vars
                .insert(k.to_string(), crate::val::Val::Raw(serde_json::json!(v)));
        }
        tc
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
        std::fs::write(dir.path().join("prompts/a.md"), "alpha").unwrap();
        std::fs::write(dir.path().join("prompts/b.md"), "beta").unwrap();
        dir
    }

    #[test]
    fn a_digest_salt_is_replaced_by_the_content_hash() {
        let dir = fixture();
        let mut tests = vec![case("$digest: prompts/a.md", &[])];
        resolve_digest_salts(&mut tests, dir.path(), &TemplateEngine::new()).unwrap();
        let salt = tests[0].cache_salt.clone().unwrap();
        assert!(salt.starts_with("blake3:"), "{salt}");
        assert!(!salt.contains("$digest"));
    }

    /// The whole point: editing one prompt must bust only the cases that use
    /// it, which is what a per-case content digest buys over a constant salt.
    #[test]
    fn editing_the_file_changes_the_salt() {
        let dir = fixture();
        let mut before = vec![case("$digest: prompts/a.md", &[])];
        resolve_digest_salts(&mut before, dir.path(), &TemplateEngine::new()).unwrap();

        std::fs::write(dir.path().join("prompts/a.md"), "alpha edited").unwrap();
        let mut after = vec![case("$digest: prompts/a.md", &[])];
        resolve_digest_salts(&mut after, dir.path(), &TemplateEngine::new()).unwrap();

        assert_ne!(before[0].cache_salt, after[0].cache_salt);
    }

    /// Templated, so a case can digest exactly the file it exercises — the
    /// thing that made consumers write a generator just to inject this field.
    #[test]
    fn the_glob_is_templated_from_the_case_vars() {
        let dir = fixture();
        let mut a = vec![case("$digest: prompts/{{ id }}.md", &[("id", "a")])];
        let mut b = vec![case("$digest: prompts/{{ id }}.md", &[("id", "b")])];
        resolve_digest_salts(&mut a, dir.path(), &TemplateEngine::new()).unwrap();
        resolve_digest_salts(&mut b, dir.path(), &TemplateEngine::new()).unwrap();
        assert_ne!(a[0].cache_salt, b[0].cache_salt);
    }

    /// Moving content between two matched files is a real edit, and hashing
    /// contents without their paths would not notice it.
    #[test]
    fn moving_content_between_matched_files_changes_the_salt() {
        let dir = fixture();
        let mut before = vec![case("$digest: prompts/*.md", &[])];
        resolve_digest_salts(&mut before, dir.path(), &TemplateEngine::new()).unwrap();

        std::fs::write(dir.path().join("prompts/a.md"), "beta").unwrap();
        std::fs::write(dir.path().join("prompts/b.md"), "alpha").unwrap();
        let mut after = vec![case("$digest: prompts/*.md", &[])];
        resolve_digest_salts(&mut after, dir.path(), &TemplateEngine::new()).unwrap();

        assert_ne!(before[0].cache_salt, after[0].cache_salt);
    }

    /// An empty match would give every such case one shared constant salt,
    /// silently collapsing the separation the salt exists to provide.
    #[test]
    fn a_glob_matching_nothing_is_an_error() {
        let dir = fixture();
        let mut tests = vec![case("$digest: prompts/missing-*.md", &[])];
        let err = resolve_digest_salts(&mut tests, dir.path(), &TemplateEngine::new()).unwrap_err();
        assert!(err.to_string().contains("matched no files"), "{err}");
    }

    /// A salt is a suite-authored path, and a suite must not read outside its
    /// own tree.
    #[test]
    fn a_traversing_glob_is_rejected() {
        let dir = fixture();
        let mut tests = vec![case("$digest: ../../../etc/*", &[])];
        assert!(resolve_digest_salts(&mut tests, dir.path(), &TemplateEngine::new()).is_err());
    }

    /// An ordinary opaque salt is untouched — this is opt-in by prefix.
    #[test]
    fn a_plain_salt_passes_through_unchanged() {
        let dir = fixture();
        let mut tests = vec![case("v1", &[])];
        resolve_digest_salts(&mut tests, dir.path(), &TemplateEngine::new()).unwrap();
        assert_eq!(tests[0].cache_salt.as_deref(), Some("v1"));
    }
}
