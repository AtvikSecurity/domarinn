//! `${env:VAR}` interpolation in provider configuration.
//!
//! Provider endpoints and credentials-adjacent settings often differ per
//! environment (a local Ollama URL, a per-developer gateway). Rather than teach
//! every provider field to read the environment, the loader rewrites `${env:…}`
//! placeholders in the **provider-bearing** subtrees of the composed YAML before
//! it is deserialized into a [`Suite`](crate::config::Suite).
//!
//! Scope is deliberately narrow. This runs over `providers[]`, `grader.provider`,
//! every assert's inner `grader.provider`, and `cache.s3` — never over test
//! `vars`. Vars keep their own, sandboxed templating (`{{ env.X }}`), so an
//! attacker-controlled fixture can never smuggle a different endpoint or key
//! name into a provider through this mechanism.
//!
//! Syntax:
//! * `${env:VAR}` — the value of `VAR`; an unset `VAR` is a hard load error.
//! * `${env:VAR:-default}` — the value of `VAR`, or `default` when unset/absent.
//! * `$${env:VAR}` — an escaped literal, rendered as the text `${env:VAR}`.
//!
//! A `${…}` that is not `${env:…}` is left untouched (so a downstream service's
//! own `${…}` placeholders in, say, an http body survive verbatim). Only the
//! `${env:` sigil is claimed here.

use serde_yaml_ng::Value as Yaml;

use crate::loader::LoadError;

/// Resolves environment-variable names to values. Injectable so tests need not
/// mutate the process environment.
pub trait EnvResolver {
    fn resolve(&self, var: &str) -> Option<String>;
}

/// The production resolver: reads the real process environment.
pub struct ProcessEnv;

impl EnvResolver for ProcessEnv {
    fn resolve(&self, var: &str) -> Option<String> {
        std::env::var(var).ok()
    }
}

/// Any `Fn(&str) -> Option<String>` is an [`EnvResolver`] — convenient for
/// tests that want to back the lookup with a fixed map.
impl<F> EnvResolver for F
where
    F: Fn(&str) -> Option<String>,
{
    fn resolve(&self, var: &str) -> Option<String> {
        self(var)
    }
}

/// Interpolate `${env:…}` placeholders in the provider-bearing subtrees of a
/// composed suite document. Mutates `raw` in place. Test `vars` are never
/// touched (see the module docs).
pub fn interpolate_env(raw: &mut Yaml, env: &impl EnvResolver) -> Result<(), LoadError> {
    if let Some(Yaml::Sequence(providers)) = raw.get_mut("providers") {
        for (i, provider) in providers.iter_mut().enumerate() {
            interp_subtree(provider, &format!("providers[{i}]"), env)?;
        }
    }

    // Top-level `grader.provider`.
    interp_grader_provider(raw.get_mut("grader"), "grader", env)?;

    // Every assert's inner `grader.provider`, in both `defaults.assert[]` and
    // inline `tests[].assert[]`.
    if let Some(asserts) = raw
        .get_mut("defaults")
        .and_then(|d| d.get_mut("assert"))
        .and_then(Yaml::as_sequence_mut)
    {
        for (j, entry) in asserts.iter_mut().enumerate() {
            interp_grader_provider(
                entry.get_mut("grader"),
                &format!("defaults.assert[{j}].grader"),
                env,
            )?;
        }
    }
    if let Some(tests) = raw.get_mut("tests").and_then(Yaml::as_sequence_mut) {
        for (i, test) in tests.iter_mut().enumerate() {
            if let Some(asserts) = test.get_mut("assert").and_then(Yaml::as_sequence_mut) {
                for (j, entry) in asserts.iter_mut().enumerate() {
                    interp_grader_provider(
                        entry.get_mut("grader"),
                        &format!("tests[{i}].assert[{j}].grader"),
                        env,
                    )?;
                }
            }
        }
    }

    // Non-secret S3 cache settings (endpoint / region / bucket / prefix).
    if let Some(s3) = raw.get_mut("cache").and_then(|c| c.get_mut("s3")) {
        interp_subtree(s3, "cache.s3", env)?;
    }

    Ok(())
}

/// Interpolate the `provider:` mapping inside a grader, if present. `grader_path`
/// is the dotted path of the grader node itself (e.g. `grader`,
/// `tests[0].assert[1].grader`).
fn interp_grader_provider(
    grader: Option<&mut Yaml>,
    grader_path: &str,
    env: &impl EnvResolver,
) -> Result<(), LoadError> {
    if let Some(provider) = grader.and_then(|g| g.get_mut("provider")) {
        interp_subtree(provider, &format!("{grader_path}.provider"), env)?;
    }
    Ok(())
}

/// Recursively interpolate every string leaf under `value`, tracking a dotted
/// `path` for error messages.
fn interp_subtree(value: &mut Yaml, path: &str, env: &impl EnvResolver) -> Result<(), LoadError> {
    match value {
        Yaml::String(s) => {
            if let Some(replaced) = interpolate_str(s, path, env)? {
                *s = replaced;
            }
            Ok(())
        }
        Yaml::Sequence(seq) => {
            for (i, item) in seq.iter_mut().enumerate() {
                interp_subtree(item, &format!("{path}[{i}]"), env)?;
            }
            Ok(())
        }
        Yaml::Mapping(map) => {
            for (k, v) in map.iter_mut() {
                let key = k.as_str().unwrap_or("?");
                interp_subtree(v, &format!("{path}.{key}"), env)?;
            }
            Ok(())
        }
        Yaml::Tagged(tagged) => interp_subtree(&mut tagged.value, path, env),
        _ => Ok(()),
    }
}

/// Interpolate a single string. Returns `Ok(None)` when the string is unchanged
/// (the common case — no `$` at all), `Ok(Some(new))` when a substitution or
/// escape was applied.
fn interpolate_str(
    s: &str,
    path: &str,
    env: &impl EnvResolver,
) -> Result<Option<String>, LoadError> {
    // Fast path: nothing to do without a `$`.
    if !s.contains('$') {
        return Ok(None);
    }

    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut changed = false;

    while i < bytes.len() {
        if bytes[i] == b'$' {
            // `$$` — an escaped dollar. Emit one `$`; the following `{…}` (if
            // any) is then ordinary text, so `$${env:X}` becomes `${env:X}`.
            if i + 1 < bytes.len() && bytes[i + 1] == b'$' {
                out.push('$');
                i += 2;
                changed = true;
                continue;
            }
            // `${env:` — our placeholder. Anything else starting with `$` is
            // emitted verbatim.
            if s[i..].starts_with("${env:") {
                let inner_start = i + 2; // just past `${`
                let close = s[inner_start..].find('}').map(|off| inner_start + off);
                let Some(close) = close else {
                    return Err(LoadError::EnvSyntax {
                        path: path.to_string(),
                        message: format!("unterminated placeholder starting at `{}`", &s[i..]),
                    });
                };
                let inner = &s[inner_start + 4..close]; // past `env:`
                let value = resolve_placeholder(inner, path, env)?;
                out.push_str(&value);
                i = close + 1;
                changed = true;
                continue;
            }
        }
        // Default: copy this byte's char through. Indexing on a UTF-8 boundary
        // is safe because we only advance past whole placeholders or single
        // ASCII bytes (`$`), and otherwise step by full chars here.
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    Ok(if changed { Some(out) } else { None })
}

/// Resolve the inside of a `${env:…}` placeholder — the text after `env:` and
/// before the closing `}` — honoring a `:-default` fallback.
fn resolve_placeholder(
    inner: &str,
    path: &str,
    env: &impl EnvResolver,
) -> Result<String, LoadError> {
    let (var, default) = match inner.split_once(":-") {
        Some((var, def)) => (var, Some(def)),
        None => (inner, None),
    };
    if var.is_empty() {
        return Err(LoadError::EnvSyntax {
            path: path.to_string(),
            message: "empty variable name in `${env:}`".to_string(),
        });
    }
    match env.resolve(var) {
        Some(v) => Ok(v),
        None => match default {
            Some(def) => Ok(def.to_string()),
            None => Err(LoadError::EnvMissing {
                path: path.to_string(),
                var: var.to_string(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A map-backed resolver, so tests never mutate the process environment.
    fn resolver(pairs: &[(&str, &str)]) -> impl EnvResolver {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |var: &str| map.get(var).cloned()
    }

    fn interp_one(s: &str, env: &impl EnvResolver) -> Result<String, LoadError> {
        Ok(interpolate_str(s, "test", env)?.unwrap_or_else(|| s.to_string()))
    }

    #[test]
    fn resolves_a_set_variable() {
        let env = resolver(&[("BASE_URL", "http://localhost:11434/v1")]);
        assert_eq!(
            interp_one("${env:BASE_URL}", &env).unwrap(),
            "http://localhost:11434/v1"
        );
    }

    #[test]
    fn resolves_amid_surrounding_text() {
        let env = resolver(&[("HOST", "example.com")]);
        assert_eq!(
            interp_one("https://${env:HOST}/v1/chat", &env).unwrap(),
            "https://example.com/v1/chat"
        );
    }

    #[test]
    fn uses_default_when_unset() {
        let env = resolver(&[]);
        assert_eq!(
            interp_one("${env:REGION:-us-east-1}", &env).unwrap(),
            "us-east-1"
        );
    }

    #[test]
    fn set_variable_wins_over_default() {
        let env = resolver(&[("REGION", "eu-west-1")]);
        assert_eq!(
            interp_one("${env:REGION:-us-east-1}", &env).unwrap(),
            "eu-west-1"
        );
    }

    #[test]
    fn empty_default_is_allowed() {
        let env = resolver(&[]);
        assert_eq!(interp_one("[${env:MAYBE:-}]", &env).unwrap(), "[]");
    }

    #[test]
    fn missing_without_default_errors_naming_var_and_path() {
        let env = resolver(&[]);
        let err = interpolate_str("${env:MISSING}", "providers[0].base_url", &env).unwrap_err();
        match &err {
            LoadError::EnvMissing { path, var } => {
                assert_eq!(path, "providers[0].base_url");
                assert_eq!(var, "MISSING");
            }
            other => panic!("expected EnvMissing, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(msg.contains("MISSING"), "{msg}");
        assert!(
            msg.contains(":-"),
            "message hints at the default form: {msg}"
        );
    }

    #[test]
    fn escaped_placeholder_is_literal() {
        let env = resolver(&[("X", "should-not-appear")]);
        assert_eq!(interp_one("$${env:X}", &env).unwrap(), "${env:X}");
    }

    #[test]
    fn unterminated_placeholder_is_a_syntax_error() {
        let env = resolver(&[]);
        let err = interpolate_str("${env:NOPE", "cache.s3.endpoint", &env).unwrap_err();
        assert!(matches!(err, LoadError::EnvSyntax { .. }), "{err:?}");
        assert!(err.to_string().contains("cache.s3.endpoint"));
    }

    #[test]
    fn empty_variable_name_is_a_syntax_error() {
        let env = resolver(&[]);
        let err = interpolate_str("${env:}", "p", &env).unwrap_err();
        assert!(matches!(err, LoadError::EnvSyntax { .. }), "{err:?}");
    }

    #[test]
    fn non_env_dollar_brace_is_left_untouched() {
        let env = resolver(&[("X", "v")]);
        // A `${...}` that isn't `${env:...}` passes through verbatim.
        assert_eq!(
            interp_one("${OTHER} and $HOME", &env).unwrap(),
            "${OTHER} and $HOME"
        );
    }

    #[test]
    fn plain_string_is_unchanged_none() {
        let env = resolver(&[]);
        assert!(interpolate_str("no placeholders here", "p", &env)
            .unwrap()
            .is_none());
    }

    #[test]
    fn walks_nested_headers_and_params() {
        let env = resolver(&[("TOKEN", "secret123"), ("MODEL", "llama3")]);
        let mut doc: Yaml = serde_yaml_ng::from_str(
            r#"
providers:
  - id: p
    type: openai
    model: "${env:MODEL}"
    base_url: "https://api/${env:MISSING:-default-host}/v1"
    headers:
      authorization: "Bearer ${env:TOKEN}"
    params:
      nested:
        deep: "m-${env:MODEL}"
"#,
        )
        .unwrap();
        interpolate_env(&mut doc, &env).unwrap();
        let p = &doc["providers"][0];
        assert_eq!(p["model"].as_str().unwrap(), "llama3");
        assert_eq!(
            p["base_url"].as_str().unwrap(),
            "https://api/default-host/v1"
        );
        assert_eq!(
            p["headers"]["authorization"].as_str().unwrap(),
            "Bearer secret123"
        );
        assert_eq!(p["params"]["nested"]["deep"].as_str().unwrap(), "m-llama3");
    }

    #[test]
    fn test_vars_are_never_interpolated() {
        let env = resolver(&[("SECRET", "leaked")]);
        let mut doc: Yaml = serde_yaml_ng::from_str(
            r#"
providers:
  - {id: p, type: openai, model: "${env:SECRET}"}
tests:
  - vars:
      user_input: "${env:SECRET}"
"#,
        )
        .unwrap();
        interpolate_env(&mut doc, &env).unwrap();
        // Provider field resolved...
        assert_eq!(doc["providers"][0]["model"].as_str().unwrap(), "leaked");
        // ...but the test var is left verbatim for the sandboxed var engine.
        assert_eq!(
            doc["tests"][0]["vars"]["user_input"].as_str().unwrap(),
            "${env:SECRET}"
        );
    }

    #[test]
    fn grader_and_cache_subtrees_are_interpolated() {
        let env = resolver(&[("GRADER_KEY", "GK"), ("S3_ENDPOINT", "http://minio:9000")]);
        let mut doc: Yaml = serde_yaml_ng::from_str(
            r#"
providers: [{id: p, type: exec, command: ["x"]}]
grader:
  provider: {type: openai, model: m, api_key_env: "${env:GRADER_KEY}"}
cache:
  backend: s3
  s3:
    bucket: b
    endpoint: "${env:S3_ENDPOINT}"
tests:
  - vars: {}
    assert:
      - type: llm-rubric
        value: "ok"
        grader:
          provider: {type: openai, model: m, api_key_env: "${env:GRADER_KEY}"}
"#,
        )
        .unwrap();
        interpolate_env(&mut doc, &env).unwrap();
        assert_eq!(
            doc["grader"]["provider"]["api_key_env"].as_str().unwrap(),
            "GK"
        );
        assert_eq!(
            doc["cache"]["s3"]["endpoint"].as_str().unwrap(),
            "http://minio:9000"
        );
        assert_eq!(
            doc["tests"][0]["assert"][0]["grader"]["provider"]["api_key_env"]
                .as_str()
                .unwrap(),
            "GK"
        );
    }
}
