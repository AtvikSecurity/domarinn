//! Validation for `providers[].fallback` and `runner.refusal_patterns`.
//!
//! A sibling of `loader_validate`, following `loader_validate_history`: these
//! checks need the *whole* provider list, so they cannot live inside the
//! per-provider loop that file already runs.
//!
//! The point of doing this at validate time is that every one of these mistakes
//! is otherwise discovered at three in the morning, mid-run, after the suite has
//! already been paid for.

use crate::config::{ProviderKind, Suite};
use crate::loader_validate::{Issue, Severity};

/// Check every fallback chain and refusal pattern in the suite.
pub(crate) fn check(suite: &Suite, issues: &mut Vec<Issue>) {
    let known: Vec<&str> = suite.providers.iter().map(|p| p.id.as_str()).collect();

    for (i, provider) in suite.providers.iter().enumerate() {
        let path = format!("providers[{i}]");
        let is_embeddings = matches!(provider.kind, ProviderKind::Embeddings { .. });

        if !provider.fallback.is_empty() && is_embeddings {
            issues.push(Issue {
                path: path.clone(),
                message: format!(
                    "provider '{}' is an embeddings provider, which is a grader helper and never \
                     a system under test, so its `fallback:` can never be reached",
                    provider.id
                ),
                severity: Severity::Error,
            });
        }

        if provider.fallback_only && is_embeddings {
            issues.push(Issue {
                path: path.clone(),
                message: format!(
                    "provider '{}' is an embeddings provider, which never forms matrix cells \
                     anyway; `fallback_only:` is for systems under test",
                    provider.id
                ),
                severity: Severity::Error,
            });
        }

        // A `fallback_only` provider nothing points at can never run at all —
        // not as a cell (the flag), not through a chain (nothing names it).
        // A warning rather than an error: the suite still runs, the same way a
        // provider excluded by every test does.
        if provider.fallback_only
            && !is_embeddings
            && !suite
                .providers
                .iter()
                .any(|p| p.fallback.iter().any(|t| t == &provider.id))
        {
            issues.push(Issue {
                path: path.clone(),
                message: format!(
                    "provider '{}' is `fallback_only`, but no other provider's `fallback:` names \
                     it, so it can never run",
                    provider.id
                ),
                severity: Severity::Warning,
            });
        }

        let mut seen: Vec<&str> = Vec::new();
        for (j, target) in provider.fallback.iter().enumerate() {
            let at = format!("{path}.fallback[{j}]");

            if target == &provider.id {
                issues.push(Issue {
                    path: at.clone(),
                    message: format!(
                        "'{target}' is this provider; a fallback must name another one"
                    ),
                    severity: Severity::Error,
                });
                continue;
            }

            let Some(other) = suite.providers.iter().find(|p| &p.id == target) else {
                issues.push(Issue {
                    path: at.clone(),
                    message: format!(
                        "no provider has id '{target}'. Configured providers: {}",
                        known.join(", ")
                    ),
                    severity: Severity::Error,
                });
                continue;
            };

            if matches!(other.kind, ProviderKind::Embeddings { .. }) {
                issues.push(Issue {
                    path: at.clone(),
                    message: format!(
                        "'{target}' is an embeddings provider, which is a grader helper and never \
                         a system under test"
                    ),
                    severity: Severity::Error,
                });
                continue;
            }

            if seen.contains(&target.as_str()) {
                issues.push(Issue {
                    path: at.clone(),
                    message: format!("'{target}' is listed twice; the second entry is a no-op"),
                    severity: Severity::Warning,
                });
            }
            seen.push(target.as_str());

            // The check that makes the no-chaining rule discoverable while the
            // suite is being written, rather than when a run quietly stops one
            // link short of where its author expected.
            if !other.fallback.is_empty() {
                issues.push(Issue {
                    path: at.clone(),
                    message: format!(
                        "'{target}' declares its own `fallback:`, which is ignored when '{target}' \
                         is reached as one — chains are not followed. List every provider you want \
                         tried directly on '{}'",
                        provider.id
                    ),
                    severity: Severity::Warning,
                });
            }
        }
    }

    // Every system under test `fallback_only` is a matrix that is provably
    // empty before a single test is read — the run-time refusal exists too
    // (for library callers), but this is the mistake `validate` is for.
    let under_test: Vec<&crate::config::Provider> = suite
        .providers
        .iter()
        .filter(|p| !matches!(p.kind, ProviderKind::Embeddings { .. }))
        .collect();
    if !under_test.is_empty() && under_test.iter().all(|p| p.fallback_only) {
        issues.push(Issue {
            path: "providers".into(),
            message: "every provider is `fallback_only`, so no cells can ever form; at least \
                      one provider must be able to run its own cells"
                .into(),
            severity: Severity::Error,
        });
    }

    if let Some(runner) = suite.runner.as_ref() {
        for (i, pattern) in runner.refusal_patterns.iter().enumerate() {
            if let Err(e) = regex::Regex::new(pattern) {
                issues.push(Issue {
                    path: format!("runner.refusal_patterns[{i}]"),
                    message: format!("not a valid regex: {e}"),
                    severity: Severity::Error,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite_from(yaml: &str) -> Suite {
        serde_yaml_ng::from_str(yaml).expect("test fixture must parse")
    }

    fn check_yaml(yaml: &str) -> Vec<Issue> {
        let mut issues = Vec::new();
        check(&suite_from(yaml), &mut issues);
        issues
    }

    const TWO: &str = r#"
version: 1
providers:
  - id: primary
    type: exec
    command: ["true"]
  - id: backup
    type: exec
    command: ["true"]
tests:
  - id: t
    assert:
      - type: contains
        value: x
"#;

    #[test]
    fn a_valid_fallback_produces_no_issue() {
        let yaml = TWO.replace("  - id: backup", "    fallback: [backup]\n  - id: backup");
        assert!(check_yaml(&yaml).is_empty(), "{:?}", check_yaml(&yaml));
    }

    #[test]
    fn an_unknown_fallback_id_is_an_error() {
        let yaml = TWO.replace("  - id: backup", "    fallback: [nope]\n  - id: backup");
        let issues = check_yaml(&yaml);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
        assert!(issues[0].message.contains("no provider has id 'nope'"));
        // The message lists what *is* configured, so the fix is one read away.
        assert!(issues[0].message.contains("primary, backup"));
    }

    #[test]
    fn a_self_referencing_fallback_is_an_error() {
        let yaml = TWO.replace("  - id: backup", "    fallback: [primary]\n  - id: backup");
        let issues = check_yaml(&yaml);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
        assert!(issues[0].message.contains("this provider"));
    }

    #[test]
    fn a_duplicate_target_warns_that_the_second_is_a_no_op() {
        let yaml = TWO.replace(
            "  - id: backup",
            "    fallback: [backup, backup]\n  - id: backup",
        );
        let issues = check_yaml(&yaml);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Warning);
        assert!(issues[0].message.contains("listed twice"));
    }

    /// The highest-value check here: chains are not followed, and a suite that
    /// assumes otherwise fails silently by stopping one link early.
    #[test]
    fn a_target_with_its_own_fallback_warns_that_chains_are_not_followed() {
        let yaml = r#"
version: 1
providers:
  - id: a
    type: exec
    command: ["true"]
    fallback: [b]
  - id: b
    type: exec
    command: ["true"]
    fallback: [c]
  - id: c
    type: exec
    command: ["true"]
tests:
  - id: t
    assert:
      - type: contains
        value: x
"#;
        let issues = check_yaml(yaml);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Warning);
        assert!(issues[0].message.contains("chains are not followed"));
    }

    #[test]
    fn an_invalid_refusal_pattern_is_an_error() {
        let yaml = format!("{TWO}runner:\n  refusal_patterns: [\"((unclosed\"]\n");
        let issues = check_yaml(&yaml);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
        assert_eq!(issues[0].path, "runner.refusal_patterns[0]");
    }

    #[test]
    fn a_referenced_fallback_only_provider_is_clean() {
        let yaml = TWO.replace(
            "  - id: backup",
            "    fallback: [backup]\n  - id: backup\n    fallback_only: true",
        );
        assert!(check_yaml(&yaml).is_empty(), "{:?}", check_yaml(&yaml));
    }

    #[test]
    fn an_unreferenced_fallback_only_provider_warns_it_can_never_run() {
        let yaml = TWO.replace("  - id: backup", "  - id: backup\n    fallback_only: true");
        let issues = check_yaml(&yaml);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].severity, Severity::Warning);
        assert!(issues[0].message.contains("can never run"));
    }

    #[test]
    fn an_all_fallback_only_suite_is_an_error() {
        let yaml = TWO
            .replace(
                "  - id: primary",
                "  - id: primary\n    fallback_only: true",
            )
            .replace("  - id: backup", "  - id: backup\n    fallback_only: true");
        let issues = check_yaml(&yaml);
        // Two "never runs" warnings plus the structural error; the error is
        // what `validate`'s exit code keys on.
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error && i.message.contains("no cells")),
            "{issues:?}"
        );
    }

    #[test]
    fn a_fallback_only_embeddings_provider_is_an_error() {
        let yaml = TWO.replace(
            "  - id: backup\n    type: exec\n    command: [\"true\"]",
            "  - id: backup\n    type: embeddings\n    model: m\n    fallback_only: true",
        );
        let issues = check_yaml(&yaml);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error && i.message.contains("fallback_only")),
            "{issues:?}"
        );
    }
}
