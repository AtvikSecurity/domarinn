//! Expanding a suite's `tests:` list into concrete test cases.
//!
//! Sources: inline cases, `file://` globs (YAML / JSON / CSV / JSONL), and
//! generator commands (deferred to the async runner). Every produced case gets
//! a stable id and the suite `defaults` merged in.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::config::{Assert, Defaults, GeneratorSpec, Suite, TestCase, TestSource};
use crate::filevars::resolve_file_vars;
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
                let loaded = load_glob(spec, base_dir)?;
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
        for tc in &mut out.tests {
            merge_defaults(tc, defaults);
        }
    }

    // Sandboxed file-content vars (`{$file: …}` / `!file`) are resolved last, so
    // fixtures pulled in by matrix axes or `defaults` are loaded too. Runs before
    // any rendering (the runner renders later), so a fixture is never a template.
    resolve_file_vars(&mut out.tests, base_dir)?;

    Ok(out)
}

fn ensure_id(tc: &mut TestCase, default: impl FnOnce() -> String) {
    if tc.id.is_none() {
        tc.id = Some(default());
    }
}

/// Merge suite `defaults` into a test case: default vars fill gaps, default
/// asserts prepend, default tags union, default threshold fills if unset.
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
}

fn load_glob(spec: &str, base_dir: &Path) -> Result<Vec<TestCase>, ResolveError> {
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

    let mut out = Vec::new();
    for path in files {
        out.extend(load_test_file(&path)?);
    }
    Ok(out)
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
/// `__assert` (a JSON assert list).
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
