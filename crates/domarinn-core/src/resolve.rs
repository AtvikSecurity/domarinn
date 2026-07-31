//! Expanding a suite's `tests:` list into concrete test cases.
//!
//! Sources: inline cases, `file://` globs (YAML / JSON / CSV / JSONL), and
//! generator commands (deferred to the async runner). Every produced case gets
//! a stable id and the suite `defaults` merged in.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::config::{Assert, Defaults, GeneratorSpec, Suite, TestCase, TestSource};
use crate::filevars::{resolve_assert_file_vals, resolve_digest_salts, resolve_file_vars};
use crate::matrix::expand_matrix;
use crate::sandbox::{self, SandboxError};
use crate::val::{desugar_tags, Val};

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("bad glob '{pattern}': {source}")]
    Glob {
        pattern: String,
        #[source]
        source: glob::PatternError,
    },
    #[error("parsing {path}: {message}")]
    Parse { path: String, message: String },
    #[error("test source '{0}' must be a file:// reference")]
    NotFileUrl(String),
    /// A `file://` reference (test-file glob or file-var fixture) that would read
    /// outside the suite directory. Closes a sandbox-escape hole.
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
}

/// The result of expanding `tests:`.
#[derive(Debug, Default)]
pub struct Expanded {
    /// Fully-resolved test cases (ids assigned, defaults merged).
    pub tests: Vec<TestCase>,
    /// Generators to run at execution time (resolved in a later phase).
    pub deferred_generators: Vec<GeneratorSpec>,
    /// Per-`file://` source accounting, so a run that resolved to nothing can
    /// name the glob that matched no files rather than reporting a total of
    /// zero. Inline sources need no entry — they are one case each by
    /// construction — and generators are accounted for after they run.
    pub globs: Vec<crate::empty_run::GlobReport>,
}

/// Expand all `tests:` sources relative to `base_dir`.
pub fn expand_tests(suite: &Suite, base_dir: &Path) -> Result<Expanded, ResolveError> {
    let mut out = Expanded::default();
    for (index, source) in suite.tests.iter().enumerate() {
        match source {
            TestSource::Inline(tc) => {
                let mut tc = tc.clone();
                ensure_id(&mut tc, || format!("inline/{index}"));
                out.tests.push(tc);
            }
            TestSource::Generator(g) => out.deferred_generators.push(g.generator.clone()),
            TestSource::Glob(spec) => {
                let (loaded, files) = load_glob(spec, base_dir)?;
                out.globs.push(crate::empty_run::GlobReport {
                    spec: spec.clone(),
                    files,
                    cases: loaded.len(),
                });
                out.tests.extend(loaded);
            }
        }
    }

    // Matrix / parameter sweeps: expand each case's cross-product of axes into
    // concrete cases (identity when it has no `matrix`). Done after case
    // production — so file-loaded cases sweep too — and before `defaults` merge.
    let mut swept = Vec::with_capacity(out.tests.len());
    for tc in out.tests.drain(..) {
        swept.extend(expand_matrix(&tc)?);
    }
    out.tests = swept;

    if let Some(defaults) = &suite.defaults {
        apply_defaults(&mut out.tests, defaults);
    }

    // Sandboxed file-content vars (`{$file: …}` / `!file`) are resolved last, so
    // fixtures pulled in by matrix axes or `defaults` are loaded too. Runs before
    // any rendering (the runner renders later), so a fixture is never a template.
    resolve_file_vars(&mut out.tests, base_dir)?;
    resolve_assert_file_vals(&mut out.tests, base_dir)?;
    resolve_digest_salts(
        &mut out.tests,
        base_dir,
        &crate::template::TemplateEngine::new(),
    )?;

    Ok(out)
}

/// Merge suite `defaults` into every case. Public because generator-produced
/// cases resolve *after* [`expand_tests`] (the generator has to run first), so
/// the runner must apply defaults to them separately.
pub fn apply_defaults(tests: &mut [TestCase], defaults: &Defaults) {
    for tc in tests {
        merge_defaults(tc, defaults);
    }
}

fn ensure_id(tc: &mut TestCase, default: impl FnOnce() -> String) {
    if tc.id.is_none() {
        tc.id = Some(default());
    }
}

/// Merge suite `defaults` into a test case: default vars fill gaps, default
/// asserts prepend, default tags union, default threshold and cache salt fill
/// if unset.
fn merge_defaults(tc: &mut TestCase, defaults: &Defaults) {
    for (k, v) in &defaults.vars {
        tc.vars.entry(k.clone()).or_insert_with(|| v.clone());
    }
    if !defaults.assert.is_empty() {
        let mut merged: Vec<Assert> = defaults.assert.clone();
        merged.append(&mut tc.assert);
        tc.assert = merged;
    }
    for tag in &defaults.tags {
        if !tc.tags.contains(tag) {
            tc.tags.push(tag.clone());
        }
    }
    if tc.threshold.is_none() {
        tc.threshold = defaults.threshold;
    }
    if tc.cache_salt.is_none() {
        tc.cache_salt = defaults.cache_salt.clone();
    }
    // Fill-if-unset, never concatenated: a case's own history is a complete
    // transcript, and prepending a suite default to it would silently change
    // what the case says it tests.
    if tc.history.is_none() {
        tc.history = defaults.history.clone();
    }
}

/// Returns the loaded cases and how many files the glob matched — the two
/// are different failures (`no such directory` versus `every file was empty`)
/// and a zero-case run wants to name which one happened.
fn load_glob(spec: &str, base_dir: &Path) -> Result<(Vec<TestCase>, usize), ResolveError> {
    let rel = spec
        .strip_prefix("file://")
        .ok_or_else(|| ResolveError::NotFileUrl(spec.to_string()))?;
    // Reject a traversal in the glob spec itself (`file://../x/*.yaml`) before
    // touching the filesystem; glob metacharacters are ordinary components here.
    sandbox::reject_bad_spec(base_dir, rel)?;
    let pattern = base_dir.join(rel);
    let pattern_str = pattern.to_string_lossy().to_string();

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let matches = glob::glob(&pattern_str).map_err(|source| ResolveError::Glob {
        pattern: pattern_str.clone(),
        source,
    })?;
    for path in matches.flatten() {
        if path.is_file() {
            // Defense in depth: a symlinked match must still resolve inside base.
            sandbox::assert_within(base_dir, &path, &path.to_string_lossy())?;
            files.push(path);
        }
    }
    files.sort();

    let file_count = files.len();
    let mut out = Vec::new();
    for path in files {
        out.extend(load_test_file(&path)?);
    }
    Ok((out, file_count))
}

/// Load one test file, dispatching on extension.
fn load_test_file(path: &Path) -> Result<Vec<TestCase>, ResolveError> {
    let text = std::fs::read_to_string(path).map_err(|source| ResolveError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "tests".to_string());
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let mut cases = match ext.as_str() {
        "yaml" | "yml" => parse_yaml_tests(&text, path)?,
        "json" => parse_json_tests(&text, path)?,
        "jsonl" | "ndjson" => parse_jsonl_tests(&text, path)?,
        "csv" => parse_delimited_tests(&text, path, b',')?,
        "tsv" => parse_delimited_tests(&text, path, b'\t')?,
        other => {
            return Err(ResolveError::Parse {
                path: path.display().to_string(),
                message: format!("unsupported test file extension '.{other}'"),
            })
        }
    };
    for (i, tc) in cases.iter_mut().enumerate() {
        ensure_id(tc, || format!("{stem}/{i}"));
    }
    Ok(cases)
}

/// Accept either a top-level sequence of tests or a mapping with a `tests:` key.
fn parse_yaml_tests(text: &str, path: &Path) -> Result<Vec<TestCase>, ResolveError> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|e| parse_err(path, e))?;
    let value = desugar_tags(value);
    let seq = match value {
        serde_yaml_ng::Value::Sequence(seq) => seq,
        serde_yaml_ng::Value::Mapping(mut map) => {
            match map.remove(serde_yaml_ng::Value::String("tests".into())) {
                Some(serde_yaml_ng::Value::Sequence(seq)) => seq,
                _ => {
                    return Err(ResolveError::Parse {
                        path: path.display().to_string(),
                        message: "expected a sequence of tests or a mapping with a 'tests' list"
                            .into(),
                    })
                }
            }
        }
        _ => {
            return Err(ResolveError::Parse {
                path: path.display().to_string(),
                message: "expected a sequence or mapping".into(),
            })
        }
    };
    seq.into_iter()
        .map(|v| serde_yaml_ng::from_value(v).map_err(|e| parse_err(path, e)))
        .collect()
}

fn parse_json_tests(text: &str, path: &Path) -> Result<Vec<TestCase>, ResolveError> {
    let value: Json = serde_json::from_str(text).map_err(|e| parse_err(path, e))?;
    let items = match value {
        Json::Array(items) => items,
        Json::Object(mut map) => match map.remove("tests") {
            Some(Json::Array(items)) => items,
            _ => {
                return Err(ResolveError::Parse {
                    path: path.display().to_string(),
                    message: "expected an array of tests or an object with a 'tests' array".into(),
                })
            }
        },
        _ => {
            return Err(ResolveError::Parse {
                path: path.display().to_string(),
                message: "expected a JSON array or object".into(),
            })
        }
    };
    items
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(|e| parse_err(path, e)))
        .collect()
}

fn parse_jsonl_tests(text: &str, path: &Path) -> Result<Vec<TestCase>, ResolveError> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|e| parse_err(path, e)))
        .collect()
}

/// Delimited tables (CSV with `,`, TSV with `\t`): header names become var keys.
/// Reserved columns: `id`, `description`, `tags` (comma-separated), `threshold`,
/// `cache_salt`, `__assert` (a JSON assert list), `__history` (a `file://` path
/// or a JSON list of `{role, content}` turns). `cache_salt` is reserved so a
/// digest column keys the cache instead of silently becoming a var (which would
/// both mis-key the entry and leak the digest to the provider).
fn parse_delimited_tests(
    text: &str,
    path: &Path,
    delimiter: u8,
) -> Result<Vec<TestCase>, ResolveError> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let headers = reader.headers().map_err(|e| parse_err(path, e))?.clone();
    let mut out = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| parse_err(path, e))?;
        let mut tc = TestCase::default();
        let mut vars: BTreeMap<String, Val> = BTreeMap::new();
        for (header, field) in headers.iter().zip(record.iter()) {
            match header {
                "id" => tc.id = Some(field.to_string()),
                "description" => tc.description = Some(field.to_string()),
                "threshold" => tc.threshold = field.trim().parse().ok(),
                "cache_salt" => tc.cache_salt = Some(field.to_string()),
                "tags" => {
                    tc.tags = field
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                }
                "__assert" => {
                    if !field.trim().is_empty() {
                        tc.assert = serde_json::from_str(field).map_err(|e| parse_err(path, e))?;
                    }
                }
                "__history" => {
                    let cell = field.trim();
                    if !cell.is_empty() {
                        // A bare `file://` path or a JSON list of turns; the
                        // JSON route reuses `HistorySpec`'s own deserializer so
                        // the error messages match the YAML forms.
                        tc.history = Some(if cell.starts_with("file://") {
                            crate::config::HistorySpec::File(cell.to_string())
                        } else {
                            serde_json::from_str(cell).map_err(|e| parse_err(path, e))?
                        });
                    }
                }
                other => {
                    vars.insert(other.to_string(), Val::Tpl(Json::String(field.to_string())));
                }
            }
        }
        tc.vars = vars;
        out.push(tc);
    }
    Ok(out)
}

fn parse_err(path: &Path, e: impl std::fmt::Display) -> ResolveError {
    ResolveError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AssertKind;

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn expands_inline_and_assigns_ids() {
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests:
  - {vars: {a: "1"}}
  - {id: named, vars: {a: "2"}}
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, Path::new(".")).unwrap();
        assert_eq!(expanded.tests.len(), 2);
        assert_eq!(expanded.tests[0].id.as_deref(), Some("inline/0"));
        assert_eq!(expanded.tests[1].id.as_deref(), Some("named"));
    }

    #[test]
    fn defaults_prepend_asserts_and_fill_vars() {
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
defaults:
  vars: {shared: "s"}
  assert: [{type: is-json}]
  tags: [all]
tests:
  - {vars: {a: "1"}, assert: [{type: contains, value: "x"}]}
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, Path::new(".")).unwrap();
        let tc = &expanded.tests[0];
        assert!(tc.vars.contains_key("shared"));
        assert!(tc.tags.contains(&"all".to_string()));
        // default assert prepends, so is-json is first, contains second.
        assert!(matches!(tc.assert[0].kind, AssertKind::IsJson));
        assert!(matches!(tc.assert[1].kind, AssertKind::Contains { .. }));
    }

    #[test]
    fn defaults_fill_case_cache_salt() {
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
defaults:
  cache_salt: "suite-wide"
tests:
  - {id: inherits, vars: {a: "1"}}
  - {id: overrides, vars: {a: "2"}, cache_salt: "own"}
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, Path::new(".")).unwrap();
        assert_eq!(expanded.tests[0].cache_salt.as_deref(), Some("suite-wide"));
        assert_eq!(expanded.tests[1].cache_salt.as_deref(), Some("own"));
    }

    /// A `cache_salt` column must key the cache, not become a var — a var would
    /// both mis-key the entry and forward the digest to the provider.
    #[test]
    fn csv_cache_salt_is_a_reserved_column() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "cases.csv",
            "id,cache_salt,question\nq1,digest-1,what is 2+2\n",
        );
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests: ["file://cases.csv"]
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        let tc = &expanded.tests[0];
        assert_eq!(tc.cache_salt.as_deref(), Some("digest-1"));
        assert!(!tc.vars.contains_key("cache_salt"));
        assert!(tc.vars.contains_key("question"));
    }

    #[test]
    fn loads_yaml_glob() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("t")).unwrap();
        write(
            &dir.path().join("t"),
            "a.yaml",
            "- {vars: {x: \"1\"}}\n- {vars: {x: \"2\"}}\n",
        );
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests:
  - "file://t/a.yaml"
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        assert_eq!(expanded.tests.len(), 2);
        assert_eq!(expanded.tests[0].id.as_deref(), Some("a/0"));
    }

    #[test]
    fn defaults_history_fills_only_unset_cases() {
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
defaults:
  history:
    - {role: user, content: "default prior"}
tests:
  - id: keeps-own
    history:
      - {role: user, content: "own prior"}
      - {role: assistant, content: "own answer"}
  - id: takes-default
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, Path::new(".")).unwrap();
        let by_id = |id: &str| {
            expanded
                .tests
                .iter()
                .find(|t| t.id.as_deref() == Some(id))
                .unwrap()
        };
        match by_id("keeps-own").history.as_ref().unwrap() {
            crate::config::HistorySpec::Inline(turns) => {
                assert_eq!(turns.len(), 2, "a case's own history must win whole");
            }
            other => panic!("expected inline turns, got {other:?}"),
        }
        match by_id("takes-default").history.as_ref().unwrap() {
            crate::config::HistorySpec::Inline(turns) => {
                assert_eq!(turns[0].content, "default prior");
            }
            other => panic!("expected the default history, got {other:?}"),
        }
    }

    #[test]
    fn yaml_test_files_carry_history() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "t.yaml",
            "- id: c1\n  history:\n    - {role: user, content: hi}\n",
        );
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests: ["file://t.yaml"]
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        assert!(
            matches!(
                expanded.tests[0].history,
                Some(crate::config::HistorySpec::Inline(_))
            ),
            "history must ride through file-loaded cases"
        );
    }

    #[test]
    fn csv_history_column_takes_json_file_and_empty_forms() {
        let text = concat!(
            "id,__history,q\n",
            "json,\"[{\"\"role\"\": \"\"user\"\", \"\"content\"\": \"\"hi\"\"}]\",next\n",
            "file,file://convo.yaml,next\n",
            "none,,next\n",
        );
        let tests = parse_delimited_tests(text, std::path::Path::new("t.csv"), b',').unwrap();
        assert!(matches!(
            tests[0].history,
            Some(crate::config::HistorySpec::Inline(_))
        ));
        assert!(
            matches!(&tests[1].history, Some(crate::config::HistorySpec::File(p)) if p == "file://convo.yaml")
        );
        assert!(tests[2].history.is_none(), "an empty cell means no history");
        assert!(
            !tests[0].vars.contains_key("__history"),
            "__history is reserved, not a var"
        );
    }

    #[test]
    fn csv_bad_history_json_names_the_file() {
        let text = "id,__history\nbroken,\"[{not json\"\n";
        let err = parse_delimited_tests(text, std::path::Path::new("cases.csv"), b',').unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cases.csv"), "error must name the file: {msg}");
    }

    #[test]
    fn loads_csv_with_reserved_columns() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "cases.csv",
            "id,tags,question\nq1,\"smoke,fast\",what is 2+2\n",
        );
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests: ["file://cases.csv"]
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        assert_eq!(expanded.tests.len(), 1);
        let tc = &expanded.tests[0];
        assert_eq!(tc.id.as_deref(), Some("q1"));
        assert_eq!(tc.tags, vec!["smoke".to_string(), "fast".to_string()]);
        assert!(tc.vars.contains_key("question"));
    }

    #[test]
    fn loads_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "cases.jsonl",
            "{\"vars\": {\"x\": \"1\"}}\n{\"vars\": {\"x\": \"2\"}}\n",
        );
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests: ["file://cases.jsonl"]
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        assert_eq!(expanded.tests.len(), 2);
    }

    #[test]
    fn generators_are_deferred() {
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests:
  - {generator: {command: ["gen.py"]}}
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, Path::new(".")).unwrap();
        assert_eq!(expanded.tests.len(), 0);
        assert_eq!(expanded.deferred_generators.len(), 1);
    }

    #[test]
    fn loads_tsv_glob() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "cases.tsv",
            "id\ttags\tquestion\nq1\tsmoke,fast\twhat is 2+2\n",
        );
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests: ["file://cases.tsv"]
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        assert_eq!(expanded.tests.len(), 1);
        let tc = &expanded.tests[0];
        assert_eq!(tc.id.as_deref(), Some("q1"));
        assert_eq!(tc.tags, vec!["smoke".to_string(), "fast".to_string()]);
        assert_eq!(
            tc.vars["question"],
            Val::Tpl(Json::String("what is 2+2".into()))
        );
    }

    #[test]
    fn expand_tests_sweeps_a_matrix_into_stable_ids() {
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests:
  - id: greet
    matrix:
      style: [terse, warm]
      temperature: [0, 1]
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, Path::new(".")).unwrap();
        let ids: Vec<&str> = expanded
            .tests
            .iter()
            .map(|t| t.id.as_deref().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                "greet[style=terse,temperature=0]",
                "greet[style=terse,temperature=1]",
                "greet[style=warm,temperature=0]",
                "greet[style=warm,temperature=1]",
            ]
        );
        // Axis values landed in vars.
        assert_eq!(
            expanded.tests[0].vars["style"],
            Val::Tpl(Json::String("terse".into()))
        );
    }

    #[test]
    fn matrix_applies_to_file_loaded_cases() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "cases.yaml", "- id: c\n  matrix: {n: [1, 2]}\n");
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests: ["file://cases.yaml"]
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        assert_eq!(expanded.tests.len(), 2);
        assert_eq!(expanded.tests[0].id.as_deref(), Some("c[n=1]"));
        assert_eq!(expanded.tests[1].id.as_deref(), Some("c[n=2]"));
    }

    #[test]
    fn expand_tests_resolves_file_vars() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "doc.txt", "hello from disk");
        write(dir.path(), "cfg.json", r#"{"k": 1}"#);
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests:
  - id: t
    vars:
      document: !file "doc.txt"
      schema: { $file: "cfg.json" }
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        let tc = &expanded.tests[0];
        assert_eq!(
            tc.vars["document"],
            Val::Tpl(Json::String("hello from disk".into()))
        );
        assert_eq!(tc.vars["schema"], Val::Tpl(serde_json::json!({ "k": 1 })));
    }

    #[test]
    fn file_var_with_raw_is_never_templated() {
        // SSTI proof: a raw fixture reaches the case verbatim.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "probe.txt", "{{7*7}}");
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests:
  - id: t
    vars:
      payload: { $file: "probe.txt", raw: true }
"#,
        )
        .unwrap();
        let expanded = expand_tests(&suite, dir.path()).unwrap();
        assert!(expanded.tests[0].vars["payload"].is_raw());
        assert_eq!(
            expanded.tests[0].vars["payload"],
            Val::Raw(Json::String("{{7*7}}".into()))
        );
    }

    #[test]
    fn file_var_escape_is_refused() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("secret.txt"), "TOP SECRET").unwrap();
        let base = parent.path().join("suite");
        std::fs::create_dir(&base).unwrap();
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests:
  - id: t
    vars:
      leak: !file "../secret.txt"
"#,
        )
        .unwrap();
        let err = expand_tests(&suite, &base).unwrap_err();
        assert!(matches!(err, ResolveError::Sandbox(_)), "{err:?}");
        assert!(err.to_string().contains("refuses to read outside"));
    }

    #[test]
    fn tests_file_glob_traversal_is_refused() {
        // A `file://../x.yaml` test-source glob must not escape the suite dir.
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("evil.yaml"), "- {vars: {}}\n").unwrap();
        let base = parent.path().join("suite");
        std::fs::create_dir(&base).unwrap();
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests: ["file://../evil.yaml"]
"#,
        )
        .unwrap();
        let err = expand_tests(&suite, &base).unwrap_err();
        assert!(matches!(err, ResolveError::Sandbox(_)), "{err:?}");
    }
}

#[cfg(test)]
mod not_sugar_tests {
    //! `type: not-<kind>` reaching every test source.
    //!
    //! One test per path that used to fail with `unknown variant
    //! `not-contains`` while the docs promised the sugar worked for any
    //! assertion type. They exercise the loaders rather than `Assert` directly,
    //! because the bug was never in the type — it was in which inputs ever
    //! reached the rewrite.

    use super::*;

    fn negated_contains(tests: &[TestCase]) -> &Assert {
        let assert = &tests[0].assert[0];
        assert!(assert.negate, "the `not-` prefix must set negate");
        assert!(
            matches!(assert.kind, crate::config::AssertKind::Contains { .. }),
            "the prefix must be stripped from the kind, got {:?}",
            assert.kind
        );
        assert
    }

    #[test]
    fn not_asserts_desugar_in_a_yaml_test_file() {
        let text = r#"
- vars: {a: 1}
  assert:
    - {type: not-contains, value: "x"}
"#;
        let tests = parse_yaml_tests(text, std::path::Path::new("t.yaml")).unwrap();
        negated_contains(&tests);
    }

    #[test]
    fn not_asserts_desugar_in_a_json_test_file() {
        let text = r#"[{"vars": {"a": 1}, "assert": [{"type": "not-contains", "value": "x"}]}]"#;
        let tests = parse_json_tests(text, std::path::Path::new("t.json")).unwrap();
        negated_contains(&tests);
    }

    #[test]
    fn not_asserts_desugar_in_a_jsonl_test_file() {
        let text = r#"{"vars": {"a": 1}, "assert": [{"type": "not-contains", "value": "x"}]}"#;
        let tests = parse_jsonl_tests(text, std::path::Path::new("t.jsonl")).unwrap();
        negated_contains(&tests);
    }

    #[test]
    fn not_asserts_desugar_in_a_csv_assert_column() {
        let text =
            "a,__assert\n1,\"[{\"\"type\"\": \"\"not-contains\"\", \"\"value\"\": \"\"x\"\"}]\"\n";
        let tests = parse_delimited_tests(text, std::path::Path::new("t.csv"), b',').unwrap();
        negated_contains(&tests);
    }

    /// An explicit `negate` alongside `not-` loses: two spellings of one intent
    /// disagreeing is a config bug, and `not-` is the more specific one.
    #[test]
    fn the_not_prefix_wins_over_an_explicit_negate_false() {
        let text = r#"
- assert:
    - {type: not-contains, value: "x", negate: false}
"#;
        let tests = parse_yaml_tests(text, std::path::Path::new("t.yaml")).unwrap();
        assert!(tests[0].assert[0].negate);
    }

    /// An unknown kind must still error, and name the kind rather than the
    /// sugared spelling, so the message points at the real mistake.
    #[test]
    fn an_unknown_not_kind_still_errors() {
        let text = r#"
- assert:
    - {type: not-frobnicate, value: "x"}
"#;
        let err = parse_yaml_tests(text, std::path::Path::new("t.yaml")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("frobnicate"),
            "error should name the unknown kind, got: {msg}"
        );
    }
}
