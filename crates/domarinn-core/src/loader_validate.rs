//! Structural validation of a loaded suite (no rendering, no provider calls).
//!
//! Split out of [`crate::loader`] to keep each module well under the file-length
//! ratchet. [`crate::loader`] re-exports [`validate`] and [`Issue`] so the
//! `domarinn_core::loader::validate` / `domarinn_core::validate` paths are
//! unchanged.
//!
//! Two checks live here that serde's `deny_unknown_fields` cannot express:
//! unknown keys inside the `flatten`ed provider/assert mappings, and unknown
//! keys inside the bare `ProviderKind` a grader carries. Both key sets come from
//! the generated JSON Schema, so they can never drift from the code.

use serde_yaml_ng::Value as Yaml;

use crate::config::Suite;

/// How much a finding costs the reader.
///
/// Exactly one axis, exactly two values. `Error` means the suite cannot run as
/// written; `Warning` means it will run and probably should not. There is no
/// `Info`: a diagnostic nobody must act on is noise, and `tracing` is where
/// noise belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Advice. The suite loads, `run` proceeds, `validate` still exits 0.
    Warning,
    /// The suite is malformed. `validate` and `run` both refuse it.
    Error,
}

impl Severity {
    /// The label the CLI prefixes a finding with.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// A structural validation problem, with a human-readable location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub path: String,
    pub message: String,
    pub severity: Severity,
}

impl Issue {
    /// An error: the default, because every check that predates warnings is
    /// one. Keeping the severity inside the constructor is what lets the
    /// existing call sites stay byte-identical.
    pub(crate) fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Issue {
            path: path.into(),
            message: message.into(),
            severity: Severity::Error,
        }
    }

    /// Advice only. See [`Severity::Warning`].
    pub(crate) fn warning(path: impl Into<String>, message: impl Into<String>) -> Self {
        Issue {
            severity: Severity::Warning,
            ..Issue::new(path, message)
        }
    }
}

/// `"{path}: {message}"` — severity is deliberately **not** rendered here.
///
/// The caller owns the label: `validate` prefixes `error: `/`warning: `, while
/// `run` routes warnings through `tracing::warn!`, where the level is already
/// the log line's own field. Baking it in would duplicate the level in logs and
/// silently rewrite every existing error line.
impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Everything [`validate`] found, in the order the checks produced it.
///
/// Deliberately not a `Vec<Issue>`, and deliberately **without** an
/// `is_empty()`. Before warnings existed, "non-empty" and "fatal" were the same
/// predicate and every caller wrote `is_empty()`. Returning a bare `Vec` again
/// would let those callers keep compiling while silently promoting every
/// warning to a hard stop — the exact failure this type exists to prevent. The
/// newtype makes each caller name the question it is asking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validation {
    issues: Vec<Issue>,
}

impl Validation {
    /// Every finding, errors and warnings interleaved in check order.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// Findings that make the suite unrunnable.
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.of(Severity::Error)
    }

    /// Findings that are advice. A caller that ignores these is still correct.
    pub fn warnings(&self) -> impl Iterator<Item = &Issue> {
        self.of(Severity::Warning)
    }

    /// The "refuse to run" predicate — the only one a runner should consult.
    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// Nothing to report at all. The predicate for "this suite is exemplary",
    /// used by the shipped-example guards, which must stay free of warnings too
    /// rather than merely free of errors.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    /// The findings as a plain vec, for tests that assert over all of them.
    ///
    /// `#[cfg(test)]` on purpose: on a bare `Vec`, `.is_empty()` compiles again
    /// and means "fatal" again, which is exactly the confusion this type exists
    /// to prevent. Production callers get [`Self::errors`], [`Self::warnings`],
    /// [`Self::has_errors`], or [`Self::is_clean`].
    #[cfg(test)]
    pub(crate) fn into_issues(self) -> Vec<Issue> {
        self.issues
    }

    fn of(&self, severity: Severity) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |i| i.severity == severity)
    }
}

/// Run structural validation that does not require rendering templates or
/// contacting providers.
///
/// A well-formed suite may still come back non-empty: [`Severity::Warning`]
/// findings are advice, and a suite carrying only those is runnable. Ask
/// [`Validation::has_errors`] to decide whether to proceed, and
/// [`Validation::is_clean`] only when "nothing to report at all" is the bar.
///
/// `raw` is the normalized YAML the suite was deserialized from (see
/// [`crate::loader::load_str_raw`] / [`crate::loader::load_file_raw`]). It is
/// needed to catch unknown keys in the `flatten`ed provider and assert mappings,
/// which serde's `deny_unknown_fields` cannot guard — an unknown key there is
/// silently dropped during deserialization, so it must be found in the raw shape.
pub fn validate(suite: &Suite, raw: &Yaml) -> Validation {
    let mut issues = Vec::new();

    check_unknown_flatten_keys(raw, &mut issues);
    check_verdict_mode(suite, &mut issues);

    if suite.version != 1 {
        issues.push(Issue::new(
            "version",
            format!("unsupported version {} (expected 1)", suite.version),
        ));
    }

    if suite.providers.is_empty() {
        issues.push(Issue::new("providers", "at least one provider is required"));
    }

    let mut seen_provider_ids = std::collections::HashSet::new();
    for (i, provider) in suite.providers.iter().enumerate() {
        if !seen_provider_ids.insert(provider.id.as_str()) {
            issues.push(Issue::new(
                format!("providers[{i}]"),
                format!("duplicate provider id '{}'", provider.id),
            ));
        }
        check_request_path(&provider.kind, &format!("providers[{i}]"), &mut issues);
    }

    let mut seen_prompt_ids = std::collections::HashSet::new();
    for (i, prompt) in suite.prompts.iter().enumerate() {
        match (&prompt.template, &prompt.messages) {
            (Some(_), Some(_)) => issues.push(Issue::new(
                format!("prompts[{i}]"),
                "set exactly one of 'template' or 'messages', not both",
            )),
            (None, None) => issues.push(Issue::new(
                format!("prompts[{i}]"),
                "set exactly one of 'template' or 'messages'",
            )),
            _ => {}
        }
        let markers = prompt
            .messages
            .iter()
            .flatten()
            .filter(|e| matches!(e, crate::config::PromptEntry::Marker(_)))
            .count();
        if markers > 1 {
            issues.push(Issue::new(
                format!("prompts[{i}]"),
                format!("{markers} `history` markers; a prompt may have at most one"),
            ));
        }
        // `messages: []` can never render a transcript. A marker-only prompt is
        // deliberately NOT flagged here: with per-case history it means "the
        // case is the transcript", and the render path errors on the specific
        // cases whose transcript still comes out empty.
        if prompt.messages.as_ref().is_some_and(|m| m.is_empty()) {
            issues.push(Issue::new(
                format!("prompts[{i}]"),
                "`messages` has no entries; add turns or a `history` marker",
            ));
        }
        if !seen_prompt_ids.insert(prompt.id.as_str()) {
            issues.push(Issue::new(
                format!("prompts[{i}]"),
                format!("duplicate prompt id '{}'", prompt.id),
            ));
        }
    }

    crate::loader_validate_history::check(suite, &mut issues);

    Validation { issues }
}

/// The set of keys a `flatten`ed config enum (Provider or Assert) accepts,
/// derived from the generated JSON Schema so it can never drift from the code.
struct VariantKeys {
    /// Keys common to every variant (the outer struct's own, un-flattened
    /// fields — e.g. `id`/`label` for a provider, `weight`/`negate` for an
    /// assert).
    common: std::collections::BTreeSet<String>,
    /// Keys accepted by each `type` variant, including `type` itself.
    by_type: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

impl VariantKeys {
    /// Read the key sets for `def` (`"Provider"` or `"Assert"`) out of the
    /// `config_schema()` output. Each variant lives under `oneOf`, keyed by the
    /// `const` value of its `type` property.
    fn from_schema(schema: &serde_json::Value, def: &str) -> VariantKeys {
        use std::collections::{BTreeMap, BTreeSet};
        let node = &schema["$defs"][def];
        let common = node
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let mut by_type = BTreeMap::new();
        if let Some(variants) = node.get("oneOf").and_then(|v| v.as_array()) {
            for variant in variants {
                let Some(props) = variant.get("properties").and_then(|p| p.as_object()) else {
                    continue;
                };
                let ty = props
                    .get("type")
                    .and_then(|t| t.get("const"))
                    .and_then(|s| s.as_str());
                if let Some(ty) = ty {
                    let keys: BTreeSet<String> = props.keys().cloned().collect();
                    by_type.insert(ty.to_string(), keys);
                }
            }
        }
        VariantKeys { common, by_type }
    }

    /// The sorted union of common + variant keys for `ty`, for error messages.
    fn allowed(&self, ty: &str) -> Vec<String> {
        let mut all: Vec<String> = self.common.iter().cloned().collect();
        if let Some(v) = self.by_type.get(ty) {
            all.extend(v.iter().cloned());
        }
        all.sort_unstable();
        all.dedup();
        all
    }
}

/// Flag unknown keys in the `flatten`ed provider and assert mappings of the raw
/// YAML — plus the bare `ProviderKind` mappings nested inside graders. serde
/// silently drops such keys (a `flatten`ed or internally-tagged enum cannot use
/// `deny_unknown_fields`), so a typo like `basurl` would otherwise go unmeasured.
///
/// The `Grader` struct itself does have `deny_unknown_fields`, so its grader-LEVEL
/// keys (`provider`/`template`/`verdict_mode`) are already rejected by serde at
/// deserialize time — even inside a `flatten`ed `llm-rubric` assert. What serde
/// cannot reject is the `provider:` value, which is a bare internally-tagged
/// `ProviderKind`; that mapping is what this walk covers, at every place a grader
/// can appear: the top-level `grader.provider` and every `llm-rubric` assert's
/// inner `grader.provider` (both `defaults.assert[]` and inline `tests[].assert[]`).
///
/// Key sets come entirely from the schema — no hand-maintained allowlist.
/// Free-form bags (`params`, an exec assert's `config`, an http provider's
/// `body`) are values, not mappings we walk, so their inner keys are never checked.
/// Reject `verdict_mode: auto`, which is in the schema and documented but which
/// the grader never reads.
///
/// The grading path is unconditionally the forced structured-verdict
/// mechanism. Accepting `auto` and silently doing something else is the same
/// class of bug as `contains-json`'s ignored `schema`: a field a user can set,
/// that changes nothing, with no way to find that out. Rejecting it is honest
/// until someone implements it — and if they do, the error moves rather than
/// a silent behavior appearing under a config nobody knew was inert.
fn check_verdict_mode(suite: &Suite, issues: &mut Vec<Issue>) {
    let mut check = |grader: &crate::config::Grader, path: &str| {
        if matches!(grader.verdict_mode, Some(crate::config::VerdictMode::Auto)) {
            issues.push(Issue::new(
                format!("{path}.verdict_mode"),
                "`auto` is not implemented; grading always uses the forced                  structured-verdict mechanism. Remove the field or set `forced`."
                    .to_string(),
            ));
        }
    };
    if let Some(g) = &suite.grader {
        check(g, "grader");
    }
    for (t, test) in suite.tests.iter().enumerate() {
        let crate::config::TestSource::Inline(tc) = test else {
            continue;
        };
        for (a, assert) in tc.assert.iter().enumerate() {
            if let crate::config::AssertKind::LlmRubric {
                grader: Some(g), ..
            } = &assert.kind
            {
                check(g, &format!("tests[{t}].assert[{a}].grader"));
            }
        }
    }
}

/// A `request.path` must be absolute.
///
/// It is joined onto `base_url` with no separator, so `chat/completions` against
/// `https://api.openai.com/v1` silently produces `.../v1chat/completions` — a
/// 404 that reads as a broken gateway rather than as the typo it is. Caught here
/// because the loader can name the provider; the join itself has no idea which
/// suite line it came from.
fn check_request_path(kind: &crate::config::ProviderKind, path: &str, issues: &mut Vec<Issue>) {
    let request = match kind {
        crate::config::ProviderKind::Anthropic { request, .. }
        | crate::config::ProviderKind::Openai { request, .. }
        | crate::config::ProviderKind::Embeddings { request, .. } => request,
        crate::config::ProviderKind::Exec { .. } | crate::config::ProviderKind::Http { .. } => {
            return
        }
    };
    let Some(declared) = request.as_ref().and_then(|r| r.path.as_deref()) else {
        return;
    };
    if !declared.starts_with('/') {
        issues.push(Issue::new(
            format!("{path}.request.path"),
            format!(
                "`{declared}` must start with `/` — it is appended to `base_url` \
                 verbatim, so a relative path would run the two together"
            ),
        ));
    }
}

fn check_unknown_flatten_keys(raw: &Yaml, issues: &mut Vec<Issue>) {
    let schema = crate::config_schema();
    let provider_keys = VariantKeys::from_schema(&schema, "Provider");
    let assert_keys = VariantKeys::from_schema(&schema, "Assert");
    // A grader's `provider:` is a bare `ProviderKind` (no wrapping `id`/`label`),
    // so its key set has no common fields — only the per-`type` variant keys.
    let provider_kind_keys = VariantKeys::from_schema(&schema, "ProviderKind");

    if let Some(providers) = raw.get("providers").and_then(Yaml::as_sequence) {
        for (i, entry) in providers.iter().enumerate() {
            check_flatten_entry(
                entry,
                &provider_keys,
                &format!("providers[{i}]"),
                "provider",
                issues,
            );
        }
    }

    // Top-level `grader.provider`.
    check_grader_provider(raw.get("grader"), &provider_kind_keys, "grader", issues);

    if let Some(asserts) = raw
        .get("defaults")
        .and_then(|d| d.get("assert"))
        .and_then(Yaml::as_sequence)
    {
        for (j, entry) in asserts.iter().enumerate() {
            let path = format!("defaults.assert[{j}]");
            check_flatten_entry(entry, &assert_keys, &path, "assert", issues);
            check_grader_provider(
                entry.get("grader"),
                &provider_kind_keys,
                &format!("{path}.grader"),
                issues,
            );
        }
    }

    if let Some(tests) = raw.get("tests").and_then(Yaml::as_sequence) {
        for (i, test) in tests.iter().enumerate() {
            // Only inline test cases carry an `assert` list; a `file://` glob is
            // a string and a generator source is a `{generator: ...}` mapping.
            if test.as_mapping().is_none() || test.get("generator").is_some() {
                continue;
            }
            if let Some(asserts) = test.get("assert").and_then(Yaml::as_sequence) {
                for (j, entry) in asserts.iter().enumerate() {
                    let path = format!("tests[{i}].assert[{j}]");
                    check_flatten_entry(entry, &assert_keys, &path, "assert", issues);
                    check_grader_provider(
                        entry.get("grader"),
                        &provider_kind_keys,
                        &format!("{path}.grader"),
                        issues,
                    );
                }
            }
        }
    }
}

/// Check the `provider:` mapping inside a grader, if present. `grader` is the raw
/// value under a `grader:` key (top-level or on an `llm-rubric` assert);
/// `grader_path` is that grader's dotted path (e.g. `grader`,
/// `tests[0].assert[1].grader`). A missing grader or missing/non-mapping provider
/// is nothing to check — serde has already rejected any grader-LEVEL typo via
/// `Grader`'s `deny_unknown_fields`, so only the nested `provider:` needs walking.
fn check_grader_provider(
    grader: Option<&Yaml>,
    keys: &VariantKeys,
    grader_path: &str,
    issues: &mut Vec<Issue>,
) {
    if let Some(provider) = grader.and_then(|g| g.get("provider")) {
        check_flatten_entry(
            provider,
            keys,
            &format!("{grader_path}.provider"),
            "provider",
            issues,
        );
    }
}

/// Check one provider/assert mapping against its schema-derived key set. The
/// mapping's `type` selects the variant; an entry with no (or an unknown) type
/// is skipped, since it would already have failed to deserialize before
/// `validate` runs.
fn check_flatten_entry(
    entry: &Yaml,
    keys: &VariantKeys,
    path: &str,
    kind: &str,
    issues: &mut Vec<Issue>,
) {
    let Some(map) = entry.as_mapping() else {
        return;
    };
    let Some(ty) = entry.get("type").and_then(|v| v.as_str()) else {
        return;
    };
    // The raw document still carries the `not-<kind>` sugar — it is desugared
    // inside `Assert`'s `Deserialize`, not in the document — so strip it before
    // selecting the variant. Without this, every negated assertion silently
    // skips its unknown-key check and a typo inside one goes unreported.
    let ty = ty.strip_prefix("not-").unwrap_or(ty);
    let Some(variant) = keys.by_type.get(ty) else {
        return;
    };
    for (k, _) in map {
        let Some(key) = k.as_str() else { continue };
        if !keys.common.contains(key) && !variant.contains(key) {
            issues.push(Issue::new(
                path.to_string(),
                format!(
                    "unknown {kind} field '{key}'; expected one of {}",
                    keys.allowed(ty).join(", ")
                ),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::load_str_raw;

    #[test]
    fn missing_providers_is_an_issue() {
        let (suite, raw) = load_str_raw("version: 1\nproviders: []\n").unwrap();
        let issues = validate(&suite, &raw).into_issues();
        assert!(issues.iter().any(|i| i.path == "providers"));
    }

    #[test]
    fn a_second_history_marker_is_an_issue() {
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers:
  - {id: p, type: openai, model: gpt-x}
prompts:
  - id: doubled
    messages:
      - history
      - {role: user, content: "hi"}
      - history
"#,
        )
        .unwrap();
        let issues = validate(&suite, &raw).into_issues();
        let hit = issues
            .iter()
            .find(|i| i.path == "prompts[0]")
            .unwrap_or_else(|| panic!("expected a prompts[0] issue, got {issues:?}"));
        assert!(
            hit.message.contains("history"),
            "message should name the marker: {}",
            hit.message
        );
    }

    /// An entirely empty `messages: []` can never render a non-empty
    /// transcript for a case without history, so say so at load. A marker-only
    /// prompt is NOT flagged: with per-case history it is a legitimate "the
    /// case is the transcript" prompt, and the render path errors on the
    /// specific cases where the transcript comes out empty.
    #[test]
    fn an_empty_messages_prompt_is_an_issue_but_marker_only_is_not() {
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers:
  - {id: p, type: openai, model: gpt-x}
prompts:
  - id: hollow
    messages: []
  - id: marker-only
    messages:
      - history
"#,
        )
        .unwrap();
        let issues = validate(&suite, &raw).into_issues();
        assert!(
            issues
                .iter()
                .any(|i| i.path == "prompts[0]" && i.message.contains("no")),
            "empty messages must be flagged: {issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.path == "prompts[1]"),
            "a marker-only prompt is legal: {issues:?}"
        );
    }

    #[test]
    fn one_history_marker_is_fine() {
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers:
  - {id: p, type: openai, model: gpt-x}
prompts:
  - id: ok
    messages:
      - {role: system, content: "sys"}
      - history
      - {role: user, content: "hi"}
"#,
        )
        .unwrap();
        let issues = validate(&suite, &raw).into_issues();
        assert!(
            !issues.iter().any(|i| i.path.starts_with("prompts")),
            "a single marker must not be flagged: {issues:?}"
        );
    }

    #[test]
    fn typo_provider_key_is_flagged_by_validate() {
        // `basurl` (a typo of `base_url`) is silently dropped by serde's
        // `flatten` of the internally-tagged ProviderKind, so it must be caught
        // by the schema-driven validate pass instead.
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers:
  - {id: p, type: openai, model: gpt-x, basurl: "http://localhost"}
"#,
        )
        .unwrap();
        let issues = validate(&suite, &raw).into_issues();
        let hit = issues
            .iter()
            .find(|i| i.path == "providers[0]")
            .unwrap_or_else(|| panic!("expected a providers[0] issue, got {issues:?}"));
        assert!(
            hit.message.contains("basurl"),
            "message should name the key: {}",
            hit.message
        );
    }

    #[test]
    fn typo_assert_key_is_flagged_by_validate() {
        // `weigth` (a typo of `weight`) inside an inline assert is dropped by
        // the flattened AssertKind; validate must flag it and name the key.
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers:
  - {id: p, type: exec, command: ["x"]}
tests:
  - vars: {}
    assert:
      - {type: contains, value: "hi", weigth: 2}
"#,
        )
        .unwrap();
        let issues = validate(&suite, &raw).into_issues();
        let hit = issues
            .iter()
            .find(|i| i.path == "tests[0].assert[0]")
            .unwrap_or_else(|| panic!("expected a tests[0].assert[0] issue, got {issues:?}"));
        assert!(
            hit.message.contains("weigth"),
            "message should name the key: {}",
            hit.message
        );
    }

    #[test]
    fn typo_grader_provider_key_is_flagged_by_validate() {
        // The top-level `grader.provider` is a bare `ProviderKind` mapping.
        // `basurl` (a typo of `base_url`) is silently dropped by the
        // internally-tagged enum — the `Grader` struct's `deny_unknown_fields`
        // only guards the grader-LEVEL keys, not the nested provider mapping.
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
grader:
  provider: {type: anthropic, model: m, basurl: "http://localhost"}
"#,
        )
        .unwrap();
        let issues = validate(&suite, &raw).into_issues();
        let hit = issues
            .iter()
            .find(|i| i.path == "grader.provider")
            .unwrap_or_else(|| panic!("expected a grader.provider issue, got {issues:?}"));
        assert!(
            hit.message.contains("basurl"),
            "message should name the key: {}",
            hit.message
        );
    }

    #[test]
    fn typo_llm_rubric_grader_provider_key_is_flagged_by_validate() {
        // An `llm-rubric` assert can carry its own inner `grader`, whose
        // `provider` is again a bare `ProviderKind`. A typo there corrupts the
        // grading model silently, so validate must flag it and name the key.
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
tests:
  - vars: {}
    assert:
      - type: llm-rubric
        value: "ok"
        grader:
          provider: {type: anthropic, model: m, basurl: "http://localhost"}
"#,
        )
        .unwrap();
        let issues = validate(&suite, &raw).into_issues();
        let hit = issues
            .iter()
            .find(|i| i.path == "tests[0].assert[0].grader.provider")
            .unwrap_or_else(|| {
                panic!("expected a tests[0].assert[0].grader.provider issue, got {issues:?}")
            });
        assert!(
            hit.message.contains("basurl"),
            "message should name the key: {}",
            hit.message
        );
    }

    #[test]
    fn typo_defaults_llm_rubric_grader_provider_key_is_flagged_by_validate() {
        // Same nested grader.provider, but reached through `defaults.assert`.
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
defaults:
  assert:
    - type: llm-rubric
      value: "ok"
      grader:
        provider: {type: openai, model: m, api_ky_env: K}
"#,
        )
        .unwrap();
        let issues = validate(&suite, &raw).into_issues();
        let hit = issues
            .iter()
            .find(|i| i.path == "defaults.assert[0].grader.provider")
            .unwrap_or_else(|| {
                panic!("expected a defaults.assert[0].grader.provider issue, got {issues:?}")
            });
        assert!(
            hit.message.contains("api_ky_env"),
            "message should name the key: {}",
            hit.message
        );
    }

    #[test]
    fn valid_grader_provider_keys_do_not_false_positive() {
        // A well-formed grader (top-level and inside llm-rubric) with a
        // free-form `params` bag must pass clean.
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["x"]}]
grader:
  provider: {type: anthropic, model: m, base_url: "http://x", api_key_env: K, params: {max_tokens: 4096}}
tests:
  - vars: {}
    assert:
      - type: llm-rubric
        value: "ok"
        grader:
          provider: {type: openai, model: m, api_key_env: K, params: {anything: 1}}
"#,
        )
        .unwrap();
        assert!(
            validate(&suite, &raw).is_clean(),
            "{:?}",
            validate(&suite, &raw)
        );
    }

    #[test]
    fn valid_provider_and_assert_keys_do_not_false_positive() {
        // Every documented key — including free-form `params` contents and the
        // desugared `not-*` assert's injected `negate` — must pass clean.
        let (suite, raw) = load_str_raw(
            r#"
version: 1
providers:
  - {id: p, type: openai, model: m, base_url: "http://x", api_key_env: K, params: {anything_here: 1}}
defaults:
  assert:
    - {type: length, max: 10}
tests:
  - vars: {}
    assert:
      - {type: not-contains, value: "x", weight: 2}
      - {type: llm-rubric, value: "ok", params: {arbitrary: true}}
"#,
        )
        .unwrap();
        assert!(
            validate(&suite, &raw).is_clean(),
            "{:?}",
            validate(&suite, &raw)
        );
    }

    #[test]
    fn duplicate_provider_ids_flagged() {
        let dup = r#"
version: 1
providers:
  - {id: dup, type: exec, command: ["a"]}
  - {id: dup, type: exec, command: ["b"]}
"#;
        let (suite, raw) = load_str_raw(dup).unwrap();
        let issues = validate(&suite, &raw).into_issues();
        assert!(issues
            .iter()
            .any(|i| i.message.contains("duplicate provider id")));
    }
}
