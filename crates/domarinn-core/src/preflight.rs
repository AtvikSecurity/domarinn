//! Checking credentials before a run spends anything.
//!
//! A missing or wrong-*shaped* credential used to be discovered by the first
//! request that 401'd. When that request is a *provider* call the damage is one
//! cell; when it is the **grader's**, every case in the suite errors, the run
//! exits 3, and the whole thing reads as an infrastructure fault rather than a
//! one-line config fix. Burning a suite's entire provider spend before learning
//! the grader key was wrong is the expensive version of that.
//!
//! This runs inside the engine rather than the CLI, so embedders and any future
//! server-side run get it, and *after* test expansion, so it also covers graders
//! that arrived from a `file://` test file or a generator — which a CLI-side
//! check structurally cannot see. Generators are local subprocesses, so running
//! them first costs nothing.
//!
//! # Only what the run will actually use
//!
//! The property that keeps this from being an annoyance. A suite that
//! configures a grader but runs a tag-filtered subset with no rubric assertions
//! must not fail on a key nothing will read.
//!
//! # Never the value
//!
//! Nothing here logs, formats, or returns a credential — only the variable
//! name, the provider id, and the dotted config path. Pinned by
//! `a_credential_value_never_appears_in_any_message`.

use crate::config::{Grader, ProviderKind, Suite, TestCase};
use crate::interp::EnvResolver;

/// One credential problem found before the run started.
#[derive(Debug, Clone, PartialEq)]
pub struct CredentialIssue {
    /// Dotted config path, e.g. `providers[1]` or `grader`.
    pub path: String,
    /// The provider id, when the path has one.
    pub provider_id: Option<String>,
    pub message: String,
}

impl std::fmt::Display for CredentialIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.provider_id {
            Some(id) => write!(f, "{} (id: {id}): {}", self.path, self.message),
            None => write!(f, "{}: {}", self.path, self.message),
        }
    }
}

/// A credential shape known to be *wrong* for a specific endpoint.
///
/// Deliberately a deny-list of exact prefixes, never an allow-list of accepted
/// shapes. An allow-list rejects every credential format that does not exist
/// yet, and this runs before a single request — where a false positive costs
/// the whole run rather than one call.
struct BadShape {
    /// The `ProviderKind` tag this applies to.
    provider_type: &'static str,
    prefix: &'static str,
    /// Only a hard failure when the request goes to this host. Elsewhere it is
    /// a warning: an internal gateway may legitimately accept the token, and
    /// domarinn cannot know.
    only_when_host_is: &'static str,
    explanation: &'static str,
}

const KNOWN_BAD: &[BadShape] = &[BadShape {
    provider_type: "anthropic",
    prefix: "sk-ant-oat",
    only_when_host_is: "api.anthropic.com",
    explanation: "an Anthropic OAuth access token, which the Messages API rejects \
                  as `x-api-key`. Use an API key from the Anthropic Console, or \
                  point `base_url` at a gateway that accepts OAuth tokens",
}];

/// Everything the run will actually read a credential for.
struct Credential<'a> {
    path: String,
    provider_id: Option<String>,
    /// Every candidate name, in the order the provider will try them.
    env_names: Vec<String>,
    provider_type: &'static str,
    base_url: Option<&'a str>,
}

impl Credential<'_> {
    /// The name(s) this credential can come from, for an error message.
    fn names(&self) -> String {
        self.env_names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn missing_message(&self) -> String {
        match self.env_names.len() {
            1 => format!(
                "environment variable {} is not set (or is empty)",
                self.names()
            ),
            _ => format!(
                "none of these environment variables are set (or all are empty): {}",
                self.names()
            ),
        }
    }
}

fn default_env_for(kind: &ProviderKind) -> Option<(&'static str, Vec<String>, Option<&str>)> {
    fn names(configured: &Option<crate::config::EnvNames>, fallback: &str) -> Vec<String> {
        match configured {
            Some(n) => n.iter().map(str::to_string).collect(),
            None => vec![fallback.to_string()],
        }
    }
    match kind {
        ProviderKind::Anthropic {
            api_key_env,
            base_url,
            ..
        } => Some((
            "anthropic",
            names(api_key_env, "ANTHROPIC_API_KEY"),
            base_url.as_deref(),
        )),
        ProviderKind::Openai {
            api_key_env,
            base_url,
            ..
        } => Some((
            "openai",
            names(api_key_env, "OPENAI_API_KEY"),
            base_url.as_deref(),
        )),
        ProviderKind::Embeddings {
            api_key_env,
            base_url,
            ..
        } => Some((
            "embeddings",
            names(api_key_env, "OPENAI_API_KEY"),
            base_url.as_deref(),
        )),
        // `exec` runs a child that owns its own credentials, and `http` templates
        // headers through `${env:...}` which the loader already resolves.
        ProviderKind::Exec { .. } | ProviderKind::Http { .. } => None,
    }
}

fn grader_credential<'a>(grader: &'a Grader, path: &str) -> Option<Credential<'a>> {
    let (provider_type, env_names, base_url) = default_env_for(&grader.provider)?;
    Some(Credential {
        path: path.to_string(),
        provider_id: None,
        env_names,
        provider_type,
        base_url,
    })
}

/// Check every credential this run will read, and return all the problems.
///
/// `selected_providers` is the post-`--provider` list, and `tests` the
/// post-filter list, so nothing is checked that the run will not touch. Both
/// halves of that are the caller's job and neither is optional — handing this
/// the unfiltered tests reintroduces exactly the annoyance the module docs
/// promise it does not have.
///
/// Borrowed rather than owned so the caller can filter without copying a large
/// suite's worth of cases just to ask about their assertions.
pub fn check(
    suite: &Suite,
    selected_providers: &[String],
    tests: &[&TestCase],
    env: &dyn EnvResolver,
) -> Vec<CredentialIssue> {
    let mut credentials: Vec<Credential> = Vec::new();

    for (i, provider) in suite.providers.iter().enumerate() {
        if !selected_providers.contains(&provider.id) {
            continue;
        }
        // The embeddings provider is a grader helper, so it is only read when a
        // `similar` assertion actually survived the filter.
        if matches!(provider.kind, ProviderKind::Embeddings { .. })
            && !any_assert(tests, |k| {
                matches!(k, crate::config::AssertKind::Similar { .. })
            })
        {
            continue;
        }
        if let Some((provider_type, env_names, base_url)) = default_env_for(&provider.kind) {
            credentials.push(Credential {
                path: format!("providers[{i}]"),
                provider_id: Some(provider.id.clone()),
                env_names,
                provider_type,
                base_url,
            });
        }
    }

    // The suite-level grader, only when an assertion that will actually read its
    // credential survived the filter. `!is_local` is too broad a proxy: it also
    // catches `exec` (a child owning its own credentials) and `similar` (the
    // embeddings provider, checked separately above), so a suite whose only
    // deferred assertions are `exec` used to be told its judge key was missing.
    // An `llm-rubric` carrying its own `grader:` block reads that one, not this.
    let needs_grader = tests.iter().flat_map(|t| t.assert.iter()).any(|a| {
        matches!(
            &a.kind,
            crate::config::AssertKind::LlmRubric { grader: None, .. }
        )
    });
    if needs_grader {
        if let Some(g) = &suite.grader {
            credentials.extend(grader_credential(g, "grader"));
        }
    }
    // Per-assert graders, which a CLI-side check could not have seen at all
    // when the test came from a generator or a `file://` glob.
    for (t, test) in tests.iter().enumerate() {
        for (a, assert) in test.assert.iter().enumerate() {
            if let crate::config::AssertKind::LlmRubric {
                grader: Some(g), ..
            } = &assert.kind
            {
                credentials.extend(grader_credential(
                    g,
                    &format!("tests[{t}].assert[{a}].grader"),
                ));
            }
        }
    }

    let mut issues = Vec::new();
    for cred in credentials {
        // An exported-but-empty variable is treated as unset: `std::env::var`
        // returns `Ok("")` for it, and an empty `x-api-key` 401s exactly like a
        // missing one.
        // First non-empty wins, matching how the provider itself resolves it.
        let value = cred
            .env_names
            .iter()
            .find_map(|name| env.resolve(name).filter(|v| !v.trim().is_empty()));
        match value {
            None => issues.push(CredentialIssue {
                path: cred.path.clone(),
                provider_id: cred.provider_id.clone(),
                message: cred.missing_message(),
            }),
            Some(value) => issues.extend(check_shape(&cred, &value)),
        }
    }
    issues
}

fn any_assert(tests: &[&TestCase], pred: impl Fn(&crate::config::AssertKind) -> bool) -> bool {
    tests
        .iter()
        .flat_map(|t| t.assert.iter())
        .any(|a| pred(&a.kind))
}

/// A known-wrong credential shape for this endpoint, if any.
fn check_shape(cred: &Credential<'_>, value: &str) -> Option<CredentialIssue> {
    let bad = KNOWN_BAD
        .iter()
        .find(|b| b.provider_type == cred.provider_type && value.starts_with(b.prefix))?;

    // Only a hard failure against the endpoint whose contract domarinn knows.
    let host = cred
        .base_url
        .and_then(|u| u.split("://").nth(1))
        .and_then(|rest| rest.split('/').next())
        .map(|h| h.split(':').next().unwrap_or(h).to_string());
    let targets_known_endpoint = match host {
        None => true, // the provider's own default base URL
        Some(h) => h == bad.only_when_host_is,
    };

    if !targets_known_endpoint {
        tracing::warn!(
            path = %cred.path,
            variables = %cred.names(),
            "credential looks like {} — accepted here only because `base_url` \
             points somewhere domarinn does not know the contract of",
            bad.explanation
        );
        return None;
    }

    Some(CredentialIssue {
        path: cred.path.clone(),
        provider_id: cred.provider_id.clone(),
        // Names the candidates rather than which one won: this is a shape
        // complaint, and the resolved name is a debug-log concern.
        message: format!(
            "the credential from {} holds {} (prefix `{}…`)",
            cred.names(),
            bad.explanation,
            bad.prefix
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct FakeEnv(BTreeMap<String, String>);

    impl EnvResolver for FakeEnv {
        fn resolve(&self, var: &str) -> Option<String> {
            self.0.get(var).cloned()
        }
    }

    fn env(pairs: &[(&str, &str)]) -> FakeEnv {
        FakeEnv(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn suite_yaml(body: &str) -> Suite {
        crate::loader::load_str(body).expect("suite loads")
    }

    const TWO_PROVIDERS: &str = r#"
version: 1
providers:
  - {id: claude, type: anthropic, model: m}
  - {id: gpt, type: openai, model: m}
"#;

    #[test]
    fn a_missing_key_is_reported_with_its_provider_and_variable() {
        let suite = suite_yaml(TWO_PROVIDERS);
        let issues = check(
            &suite,
            &["claude".into()],
            &[],
            &env(&[("OPENAI_API_KEY", "sk-x")]),
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].provider_id.as_deref(), Some("claude"));
        assert!(issues[0].message.contains("ANTHROPIC_API_KEY"));
    }

    /// `std::env::var` returns `Ok("")` for an exported-but-empty variable, and
    /// an empty `x-api-key` 401s exactly like a missing one.
    #[test]
    fn an_empty_key_counts_as_missing() {
        let suite = suite_yaml(TWO_PROVIDERS);
        let issues = check(
            &suite,
            &["claude".into()],
            &[],
            &env(&[("ANTHROPIC_API_KEY", "   ")]),
        );
        assert_eq!(issues.len(), 1);
    }

    /// All of them, not the first — otherwise fixing one key just reveals the
    /// next, one run at a time.
    #[test]
    fn every_missing_credential_is_reported() {
        let suite = suite_yaml(TWO_PROVIDERS);
        let issues = check(&suite, &["claude".into(), "gpt".into()], &[], &env(&[]));
        assert_eq!(issues.len(), 2);
    }

    /// The property that keeps this from being an annoyance.
    #[test]
    fn a_filtered_out_provider_is_not_checked() {
        let suite = suite_yaml(TWO_PROVIDERS);
        let issues = check(
            &suite,
            &["gpt".into()],
            &[],
            &env(&[("OPENAI_API_KEY", "sk-x")]),
        );
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn a_grader_key_is_only_required_when_a_deferred_assert_survives() {
        let suite = suite_yaml(
            r#"
version: 1
providers:
  - {id: e, type: exec, command: ["true"]}
grader:
  provider: {type: anthropic, model: judge}
"#,
        );
        // No tests carrying a rubric: nothing will read the grader's key.
        assert!(check(&suite, &["e".into()], &[], &env(&[])).is_empty());

        let with_rubric: Vec<TestCase> = serde_json::from_value(serde_json::json!([
            {"assert": [{"type": "llm-rubric", "value": "good"}]}
        ]))
        .unwrap();
        let with_rubric: Vec<&TestCase> = with_rubric.iter().collect();
        let issues = check(&suite, &["e".into()], &with_rubric, &env(&[]));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "grader");
    }

    #[test]
    fn an_oauth_token_against_the_official_api_is_rejected() {
        let suite = suite_yaml(TWO_PROVIDERS);
        let issues = check(
            &suite,
            &["claude".into()],
            &[],
            &env(&[("ANTHROPIC_API_KEY", "sk-ant-oat01-NOT-REAL")]),
        );
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("OAuth"));
    }

    /// A gateway may legitimately accept the token, and domarinn does not know
    /// its contract — so this warns rather than failing the run.
    #[test]
    fn an_oauth_token_against_a_custom_base_url_is_not_an_error() {
        let suite = suite_yaml(
            r#"
version: 1
providers:
  - {id: claude, type: anthropic, model: m, base_url: "https://gateway.internal/v1"}
"#,
        );
        let issues = check(
            &suite,
            &["claude".into()],
            &[],
            &env(&[("ANTHROPIC_API_KEY", "sk-ant-oat01-NOT-REAL")]),
        );
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// Open-set safety: the deny-list must not reject a credential format that
    /// does not exist yet.
    #[test]
    fn an_ordinary_and_an_unknown_future_key_shape_are_both_accepted() {
        let suite = suite_yaml(TWO_PROVIDERS);
        for value in ["sk-ant-api03-EXAMPLE", "sk-ant-zz99-FROM-THE-FUTURE"] {
            let issues = check(
                &suite,
                &["claude".into()],
                &[],
                &env(&[("ANTHROPIC_API_KEY", value)]),
            );
            assert!(issues.is_empty(), "{value} should be accepted: {issues:?}");
        }
    }

    /// The test that stops a future refactor leaking a key into a CI log.
    #[test]
    fn a_credential_value_never_appears_in_any_message() {
        let secret = "sk-ant-oat01-THE-ACTUAL-SECRET-VALUE";
        let suite = suite_yaml(TWO_PROVIDERS);
        let issues = check(
            &suite,
            &["claude".into()],
            &[],
            &env(&[("ANTHROPIC_API_KEY", secret)]),
        );
        assert_eq!(issues.len(), 1);
        for rendered in [issues[0].message.clone(), issues[0].to_string()] {
            assert!(
                !rendered.contains(secret),
                "a credential value leaked into: {rendered}"
            );
        }
    }
}
