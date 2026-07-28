//! Component digests: *what* changed between two runs, not merely *that*
//! something did.
//!
//! [`crate::result::RunResult::config_digest`] is one hash over the whole
//! serialized suite. A prompt edit, a model bump and a typo in a description all
//! flip it identically, so the only question it can answer is "did anything
//! change". These answer which part.
//!
//! Two granularities, and both are needed:
//!
//! - **Run level** ([`ConfigDigests`]) — five hashes on the run document,
//!   promoted to indexed columns. The runs list and the status surface need
//!   "the prompts changed" from the row itself; deriving it would mean fetching
//!   a config snapshot per run.
//! - **Case level** (`prompt_digest`, `provider_digest`, `assert_digest`) — a
//!   suite-level prompts digest only says *some* prompt moved, which is noise
//!   for a case that does not use it. These are exact.
//!
//! Every digest is `blake3:<hex>` over [`crate::cache::canonical_json`], the
//! same construction `config_digest` already uses. Deliberately not the
//! `sha256:` prefix the cache uses: a `sha256:`-shaped string is a valid cache
//! key, and someone would eventually try to fetch one.

use serde_json::Value as Json;

use crate::config::{Suite, TestCase};
use crate::provider::ProviderRequest;
use crate::result::{AssertResult, ConfigDigests};

fn digest(value: &Json) -> String {
    format!(
        "blake3:{}",
        blake3::hash(crate::cache::canonical_json(value).as_bytes()).to_hex()
    )
}

/// Component digests for a suite plus its fully expanded tests.
///
/// `tests` must be the **expanded** set — after globs, generators and the
/// `defaults` merge — and the **whole** set, before any `--tag`/`--filter`
/// narrowing. Both matter:
///
/// - Hashing the authored `Vec<TestSource>` is worthless for the two most
///   common setups: a `file://tests/*.yaml` glob string never changes when the
///   files behind it do, and a generator command never changes when its output
///   does.
/// - Hashing the filtered subset would make `--tag smoke` and a full run of an
///   identical suite disagree, so every filtered CI job would read as "the tests
///   changed". Filters are already recorded separately in
///   [`crate::result::FilterSpec`].
pub fn config_digests(suite: &Suite, tests: &[TestCase]) -> ConfigDigests {
    let (test_bodies, assert_bodies) = split_tests(tests);
    ConfigDigests {
        prompts: Some(digest(&to_value(&suite.prompts))),
        providers: Some(digest(&to_value(&suite.providers))),
        tests: Some(digest(&test_bodies)),
        asserts: Some(digest(&assert_bodies)),
        grader: Some(digest(&to_value(&suite.grader))),
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Json {
    serde_json::to_value(value).unwrap_or(Json::Null)
}

/// Split the expanded tests into "what is asked" and "how it is graded".
///
/// The test half is produced by *subtraction* — serialize the whole `TestCase`
/// and remove the `assert` key — rather than by listing the fields to keep.
/// `TestCase` already has a dozen fields and will grow; subtraction covers a new
/// one automatically, whereas a projection would silently drop it out of the
/// digest and make a real change invisible.
///
/// Both halves are sorted by test id so a pure reorder of a suite is not
/// reported as a change.
fn split_tests(tests: &[TestCase]) -> (Json, Json) {
    let mut bodies: Vec<(String, Json)> = Vec::with_capacity(tests.len());
    let mut asserts: Vec<(String, Json)> = Vec::with_capacity(tests.len());

    for test in tests {
        let id = test.id.clone().unwrap_or_default();
        let mut value = to_value(test);
        let assert = value
            .as_object_mut()
            .and_then(|o| o.remove("assert"))
            .unwrap_or(Json::Null);
        bodies.push((id.clone(), value));
        asserts.push((id, assert));
    }
    bodies.sort_by(|a, b| a.0.cmp(&b.0));
    asserts.sort_by(|a, b| a.0.cmp(&b.0));

    let as_json = |rows: Vec<(String, Json)>| {
        Json::Array(
            rows.into_iter()
                .map(|(id, body)| serde_json::json!({ "id": id, "body": body }))
                .collect(),
        )
    };
    (as_json(bodies), as_json(asserts))
}

/// Identity of what a case actually asked the model: the rendered prompt, the
/// rendered vars, and any per-case cache salt.
///
/// Deliberately **not** the provider fingerprint — that is
/// [`provider_digest`]'s job. Folding them together (as the cache key does)
/// would make a model bump read as "the prompt changed", conflating exactly the
/// two things this exists to separate.
///
/// Also deliberately **not** `repeat`: trials of one cell share an input by
/// construction, which is the entire point of `--repeat`, and `case_key`
/// already separates them.
///
/// Carries the same caveat as the cache key: an `exec` system under test that
/// resolves its own prompt from the test id sees different inputs for two cases
/// that look identical here. `cache_salt` is the supported way to separate them,
/// and it is a member below.
pub fn prompt_digest(req: &ProviderRequest) -> String {
    let prompt = req
        .prompt
        .as_ref()
        .map(|p| serde_json::to_value(p).unwrap_or(Json::Null))
        .unwrap_or(Json::Null);
    digest(&serde_json::json!({
        "prompt": prompt,
        "vars": Json::Object(req.vars.clone().into_iter().collect()),
        "params": Json::Object(req.params.clone()),
        // Unlike the cache key this is inserted unconditionally: a brand-new
        // digest has no previously-written values to stay compatible with, so
        // the conditional-insert dance would be complexity for nothing.
        "case_salt": req.case_salt.clone().map(Json::String).unwrap_or(Json::Null),
    }))
}

/// Identity of the model and its settings, from the provider's own fingerprint
/// (type, model, base url, params, cache salt — secrets excluded by
/// construction, and a hash could not leak them anyway).
pub fn provider_digest(fingerprint: &Json) -> String {
    digest(fingerprint)
}

/// Identity of a case's grading definition: the authored criteria, weights and
/// pass threshold, never the outcome. Changes exactly when the goalposts move.
///
/// `threshold` is a member even though it lives on `TestCase` rather than on any
/// assert, because it decides the verdict: `case_verdict` passes a case iff the
/// weighted mean reaches it. Leaving it out made a threshold edit invisible to
/// every axis, so a case flipping Fail→Pass on an unchanged output was reported
/// as [`crate::diff::CaseChange::UnstableGrader`] — a false accusation against
/// the grader for a change the author made deliberately.
///
/// `None` when any assert lacks `criteria` — pre-v2.1 stored blobs — because a
/// digest over a partial definition is a lie, and comparing it to a complete one
/// would report a change that never happened.
///
/// Member digests are sorted, so a pure reorder is not a change: local asserts
/// always evaluate before deferred ones regardless of authored order, and the
/// verdict is a weighted mean, so order carries no meaning to preserve.
pub fn assert_digest(asserts: &[AssertResult], threshold: Option<f64>) -> Option<String> {
    let mut members: Vec<String> = Vec::with_capacity(asserts.len());
    for a in asserts {
        // `criteria` omits `weight` (it is a sibling field), but weight moves
        // the verdict, so it has to be hashed explicitly.
        let criteria = a.criteria.as_ref()?;
        members.push(digest(&serde_json::json!({
            "kind": a.kind.as_str(),
            "weight": a.weight,
            "criteria": criteria,
        })));
    }
    members.sort();
    Some(digest(&serde_json::json!({
        "asserts": members,
        "threshold": threshold,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asserts::AssertName;
    use crate::result::AssertStatus;

    fn suite_of(yaml: &str) -> Suite {
        serde_yaml_ng::from_str(yaml).expect("suite parses")
    }

    const BASE: &str = r#"
version: 1
providers:
  - id: p
    type: exec
    command: ["true"]
prompts:
  - id: main
    template: "hello {{ name }}"
"#;

    fn tests_of(ids: &[&str]) -> Vec<TestCase> {
        ids.iter()
            .map(|id| TestCase {
                id: Some((*id).to_string()),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn identical_suites_digest_identically() {
        let a = config_digests(&suite_of(BASE), &tests_of(&["t1", "t2"]));
        let b = config_digests(&suite_of(BASE), &tests_of(&["t1", "t2"]));
        assert_eq!(a.prompts, b.prompts);
        assert_eq!(a.providers, b.providers);
        assert_eq!(a.tests, b.tests);
    }

    /// The whole point: a prompt edit must move the prompts digest and *nothing
    /// else*, so the UI can say "the prompts changed" rather than "config
    /// changed".
    #[test]
    fn editing_a_prompt_moves_only_the_prompts_digest() {
        let before = config_digests(&suite_of(BASE), &tests_of(&["t1"]));
        let after = config_digests(
            &suite_of(&BASE.replace("hello {{ name }}", "hi {{ name }}")),
            &tests_of(&["t1"]),
        );
        assert_ne!(before.prompts, after.prompts);
        assert_eq!(before.providers, after.providers);
        assert_eq!(before.tests, after.tests);
        assert_eq!(before.asserts, after.asserts);
    }

    #[test]
    fn changing_a_provider_moves_only_the_providers_digest() {
        let before = config_digests(&suite_of(BASE), &tests_of(&["t1"]));
        let after = config_digests(
            &suite_of(&BASE.replace(r#"["true"]"#, r#"["false"]"#)),
            &tests_of(&["t1"]),
        );
        assert_ne!(before.providers, after.providers);
        assert_eq!(before.prompts, after.prompts);
    }

    /// Reordering a suite is not a change to it.
    #[test]
    fn reordering_tests_does_not_move_the_tests_digest() {
        let a = config_digests(&suite_of(BASE), &tests_of(&["t1", "t2", "t3"]));
        let b = config_digests(&suite_of(BASE), &tests_of(&["t3", "t1", "t2"]));
        assert_eq!(a.tests, b.tests);
    }

    #[test]
    fn adding_a_test_moves_the_tests_digest() {
        let a = config_digests(&suite_of(BASE), &tests_of(&["t1"]));
        let b = config_digests(&suite_of(BASE), &tests_of(&["t1", "t2"]));
        assert_ne!(a.tests, b.tests);
    }

    fn assert_result(kind: AssertName, weight: f64, criteria: Json) -> AssertResult {
        AssertResult {
            kind,
            status: AssertStatus::Pass,
            score: 1.0,
            weight,
            reason: String::new(),
            details: None,
            criteria: Some(criteria),
            cached: false,
            cost_usd: None,
        }
    }

    #[test]
    fn assert_digest_ignores_order_but_not_content() {
        let a = assert_result(AssertName::Contains, 1.0, serde_json::json!({"value": "x"}));
        let b = assert_result(AssertName::Regex, 1.0, serde_json::json!({"value": "y"}));
        assert_eq!(
            assert_digest(&[a.clone(), b.clone()], None),
            assert_digest(&[b.clone(), a.clone()], None)
        );

        let c = assert_result(AssertName::Regex, 1.0, serde_json::json!({"value": "z"}));
        assert_ne!(
            assert_digest(&[a.clone(), b], None),
            assert_digest(&[a, c], None)
        );
    }

    /// `criteria` omits weight, so hashing it alone would miss a change that
    /// genuinely moves the verdict.
    #[test]
    fn assert_digest_covers_weight() {
        let light = assert_result(AssertName::Contains, 1.0, serde_json::json!({"value": "x"}));
        let heavy = assert_result(AssertName::Contains, 3.0, serde_json::json!({"value": "x"}));
        assert_ne!(assert_digest(&[light], None), assert_digest(&[heavy], None));
    }

    /// A partial definition must not produce a digest that looks authoritative.
    #[test]
    fn assert_digest_is_none_when_criteria_are_missing() {
        let mut bare = assert_result(AssertName::Contains, 1.0, Json::Null);
        bare.criteria = None;
        assert_eq!(assert_digest(&[bare], None), None);
    }

    /// The threshold decides the verdict, so editing it is a change to the
    /// grading definition. Without this the edit was invisible to every axis
    /// and a case flipping on an unchanged output was blamed on the grader.
    #[test]
    fn assert_digest_covers_the_pass_threshold() {
        let a = assert_result(AssertName::Contains, 1.0, serde_json::json!({"value": "x"}));
        assert_ne!(
            assert_digest(std::slice::from_ref(&a), Some(0.8)),
            assert_digest(&[a], Some(0.5))
        );
    }

    #[test]
    fn assert_digest_of_no_asserts_is_stable() {
        assert_eq!(assert_digest(&[], None), assert_digest(&[], None));
        assert!(assert_digest(&[], None).is_some());
    }
}
