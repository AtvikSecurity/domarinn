//! Unit tests for [`super`] (the ≤0.4.x key shapes). Split out of
//! `cache_migrate.rs` via `#[path]` to keep that file under the repo's
//! 1000-line source cap; this is still that module's private child
//! (`use super::*`), so it reaches the frozen private helpers directly.
//!
//! Every test here describes a store in the wild rather than a design. The
//! golden literals are the load-bearing ones: a relative assertion still passes
//! when a whole shape moves together, which is exactly the failure that strands
//! a cache, and it is silent.

use super::*;
use crate::cache_adopt::{MigrationProbe, PROBE_BUDGET};
use crate::provider::{Provider, TestMeta};
use std::collections::BTreeMap;

fn command(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

// ── The frozen 7-part key ────────────────────────────────────────────────
//
// These moved here with `provider_cache_key` when 0.5.0 took the live path
// off it. They are no longer describing a design; they are describing a
// store in the wild, so the ones that survived the move are the ones whose
// failure would mean a stranded cache.

fn req(var: &str) -> ProviderRequest {
    salted(var, None)
}

fn salted(var: &str, case_salt: Option<&str>) -> ProviderRequest {
    let mut vars = BTreeMap::new();
    vars.insert("x".to_string(), Json::String(var.into()));
    ProviderRequest {
        tools: Vec::new(),
        prompt: None,
        vars,
        params: serde_json::Map::new(),
        test: TestMeta::default(),
        case_salt: case_salt.map(String::from),
    }
}

fn fp() -> Json {
    serde_json::json!({"type": "exec"})
}

/// The golden literal for the frozen shape itself.
///
/// Every other test in this file compares two keys, so all of them would
/// still pass if the whole composite moved — which is precisely the failure
/// that strands a store, and it is silent. This one is a magic constant on
/// purpose: it is the key a 0.4.x domarinn wrote for these inputs, and if it
/// changes, the migration reads nothing.
#[test]
fn golden_seven_part_key() {
    assert_eq!(
        legacy_provider_key(&fp(), &req("a"), 0).0,
        "sha256:0f1db1256de263796a24c8e28cdc00f746a3b633e53a9757fffb66089d4f7fc5"
    );
}

/// The same, per provider kind, over the fingerprint each one published in
/// ≤0.4.0 — the shape [`crate::provider::Provider::legacy_fingerprints`]
/// leads with. A change to any provider's `fingerprint()` breaks its own pin
/// test *and* this one, which is the point: the pin says "the shape is
/// stable", this says "and the store that shape keyed is still reachable".
#[test]
fn golden_key_per_provider_kind() {
    let openai = crate::openai::OpenAiProvider::new("p", "gpt-x", None, None, None, None);
    let anthropic =
        crate::anthropic::AnthropicProvider::new("p", "claude-x", None, None, None, None);
    let http = crate::http_provider::HttpProvider::new(
        "p",
        "https://sut.test/generate",
        None,
        BTreeMap::new(),
        None,
        None,
    );
    let exec = crate::exec_provider::ExecProvider::new(
        "p",
        command(&["./sut"]),
        Default::default(),
        None,
        Some("v1".into()),
        None,
    );

    for (kind, provider) in [
        ("openai", &openai as &dyn Provider),
        ("anthropic", &anthropic),
        ("http", &http),
        ("exec", &exec),
    ] {
        let key = legacy_provider_key(&provider.fingerprint(), &req("a"), 0);
        let expected = match kind {
            "openai" => "sha256:3eab215ad9714bb3de737c39ee2ffd4bc10a6ca3559827e12f3d51752bc65ba5",
            "anthropic" => {
                "sha256:201a83d6a3f05e9ff211272aa914f29a5dffb7adb6ce062e8636c98f5d62965f"
            }
            "http" => "sha256:b0e9e3d55ddcd90bfc8d74ac77b584be21f5fb1c1126eeca5af74db285769e5a",
            _ => "sha256:8f4c04d03bce936e39fab440d6dab66fe54dda15566670b212dd5e804ff51124",
        };
        assert_eq!(key.0, expected, "{kind}: the ≤0.4.x key moved");
    }
}

/// The load-bearing backward-compatibility rule of the old shape: an
/// unsalted case hashed exactly like the pre-`case_salt` object, because the
/// member was inserted rather than defaulted to null. An entry written
/// before the field existed is reachable only while this holds.
#[test]
fn the_conditional_members_are_absent_rather_than_null() {
    let before_case_salt = CacheKey::compute(&serde_json::json!({
        "fingerprint": fp(),
        "prompt": Json::Null,
        "vars": {"x": "a"},
        "params": {},
        "repeat": 0,
    }));
    assert_eq!(legacy_provider_key(&fp(), &req("a"), 0), before_case_salt);

    // And the same for `tools`: an empty declaration is the absence of one.
    let mut empty_tools = req("a");
    empty_tools.tools = Vec::new();
    assert_eq!(
        legacy_provider_key(&fp(), &empty_tools, 0),
        before_case_salt
    );
}

/// A set salt separates, an empty one is a real value, and neither is
/// normalized away.
#[test]
fn a_case_salt_separates_and_an_empty_one_is_not_unset() {
    assert_ne!(
        legacy_provider_key(&fp(), &salted("a", None), 0),
        legacy_provider_key(&fp(), &salted("a", Some("d1")), 0)
    );
    assert_ne!(
        legacy_provider_key(&fp(), &salted("a", Some("d1")), 0),
        legacy_provider_key(&fp(), &salted("a", Some("d2")), 0)
    );
    assert_ne!(
        legacy_provider_key(&fp(), &salted("a", Some("")), 0),
        legacy_provider_key(&fp(), &salted("a", None), 0)
    );
}

/// Declaring a tool moved the old key too — so an entry written by a suite
/// with `tools:` is only adoptable if that stays true.
#[test]
fn declared_tools_moved_the_key() {
    let with_tools = |names: &[&str]| {
        let mut r = req("a");
        r.tools = names
            .iter()
            .map(|n| crate::config::ToolDef {
                name: (*n).to_string(),
                description: None,
                input_schema: None,
            })
            .collect();
        r
    };
    assert_ne!(
        legacy_provider_key(&fp(), &req("a"), 0),
        legacy_provider_key(&fp(), &with_tools(&["get_weather"]), 0)
    );
    assert_ne!(
        legacy_provider_key(&fp(), &with_tools(&["get_weather"]), 0),
        legacy_provider_key(&fp(), &with_tools(&["get_weather", "get_time"]), 0)
    );
}

/// The test id never entered the old key, so two cases with identical vars
/// shared one ≤0.4.x entry — the shape of an `exec` suite whose system under
/// test resolves its own prompt from the test id. Recorded because adoption
/// inherits it: those two cases still share the *new* entry the first of
/// them re-files.
#[test]
fn the_test_id_was_never_keyed() {
    let mut a = req("same");
    a.test = TestMeta {
        id: "case-a".into(),
        tags: vec![],
    };
    let mut b = req("same");
    b.test = TestMeta {
        id: "case-b".into(),
        tags: vec![],
    };
    assert_eq!(
        legacy_provider_key(&fp(), &a, 0),
        legacy_provider_key(&fp(), &b, 0)
    );
}

/// The point of the ordering: the shape most likely to be present is probed
/// first, so a store written by the previous release is found in one lookup.
#[test]
fn shapes_are_ordered_newest_first() {
    let fps = legacy_exec_fingerprints(&command(&["./sut"]), None, Some("v1"), None);
    assert_eq!(fps.len(), 4);
    assert!(fps[0].get("program").is_some() && fps[0].get("env").is_some());
    assert!(fps[2].get("program").is_some() && fps[2].get("env").is_none());
    assert!(fps[3].get("program").is_none());
}

/// A provider that declares `env` must not be offered the two shapes that
/// predate `env` being keyed at all.
///
/// The store in the wild: a suite whose exec provider picks its backend with
/// `env: {MODEL_ENDPOINT: …}`, run against a cache carried across versions.
/// Those two shapes have nowhere to put the digest, so every declared value
/// recomputes the same probe — point the suite at a different endpoint and it
/// adopts the old endpoint's answers, silently, for as long as the store has
/// pre-0.3.1 ancestors to offer. Dropping them costs a re-run; keeping them
/// fabricates the comparison.
#[test]
fn a_declared_env_drops_the_shapes_that_could_not_carry_it() {
    let digest = "blake3:0123456789abcdef";
    let declared = legacy_exec_fingerprints(&command(&["./sut"]), Some(digest), Some("v1"), None);

    assert_eq!(
        declared.len(),
        2,
        "only the two shapes that carry `env` may be probed, got {declared:#?}"
    );
    for shape in &declared {
        assert_eq!(
            shape.get("env"),
            Some(&Json::String(digest.into())),
            "a probed shape must key on the declared environment"
        );
    }

    // The env-less shapes are not deleted, only withheld: a provider declaring
    // nothing is the case they exist for, and its 0.3.0/0.2.x entries stay
    // reachable.
    let undeclared = legacy_exec_fingerprints(&command(&["./sut"]), None, Some("v1"), None);
    assert_eq!(undeclared.len(), 4);
    assert_eq!(
        undeclared[2].get("env"),
        None,
        "the withheld shapes are unchanged, and still have no `env` member"
    );
}

/// The invariant, stated where it is cheapest to check: two declared values
/// share no probe at all — not the live key, and not any historical shape.
#[test]
fn two_declared_env_values_share_no_probe() {
    let probes = |digest: &str| {
        legacy_exec_fingerprints(&command(&["./sut"]), Some(digest), None, None)
            .into_iter()
            .map(|fp| legacy_provider_key(&fp, &req("a"), 0))
            .collect::<Vec<_>>()
    };
    let (a, b) = (probes("blake3:aaaa"), probes("blake3:bbbb"));
    assert!(!a.is_empty());
    for key in &a {
        assert!(
            !b.contains(key),
            "a probe is shared between two declared environments: {key}"
        );
    }
}

/// …and the newest shape of all is the one 0.4.0 shipped: the provider's own
/// current fingerprint. Every provider now has at least that one, where
/// before 0.5.0 the three network kinds had no history at all — their key
/// had never moved, so there was nothing to adopt. Now it has.
#[test]
fn the_current_fingerprint_leads_the_probe_list() {
    let exec = crate::exec_provider::ExecProvider::new(
        "p",
        command(&["./sut"]),
        Default::default(),
        None,
        Some("v1".into()),
        None,
    );
    let shapes = exec.legacy_fingerprints();
    assert_eq!(
        shapes.len(),
        5,
        "the 0.4.0 shape plus four older generations"
    );
    assert_eq!(shapes[0], exec.fingerprint());
    assert_eq!(
        shapes[1..],
        legacy_exec_fingerprints(&command(&["./sut"]), None, Some("v1"), None)[..]
    );

    let anthropic =
        crate::anthropic::AnthropicProvider::new("p", "claude-x", None, None, None, None);
    assert_eq!(
        anthropic.legacy_fingerprints(),
        vec![anthropic.fingerprint()]
    );
}

/// No legacy key may equal the live key for the same call, or a probe would
/// re-read the key that just missed and the migration would be a no-op that
/// costs a lookup.
///
/// Retargeted in 0.5.0: the old version compared *fingerprints*, which was
/// the right question while the fingerprint was half the key. Now the live
/// key hashes the canonical request instead, so the honest comparison is
/// key-to-key — and it holds structurally, since `request` is not a member of
/// the frozen object and `fingerprint` is not a member of the live one.
#[test]
fn no_legacy_key_equals_the_live_key_for_one_call() {
    // A checkout where `./sut` resolves, because that is what separates the
    // two `program` flavours: with nothing on disk both walk to `[]` and the
    // 0.3.1 and 0.3.0 shapes collapse into one key. That costs a redundant
    // lookup for a command naming nothing readable — which under the old
    // rules was never cacheable, so it has nothing to adopt anyway.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sut"), "#!/bin/sh\necho v1").unwrap();
    let p = crate::exec_provider::ExecProvider::new(
        "p",
        command(&["./sut"]),
        Default::default(),
        None,
        Some("v1".into()),
        Some(dir.path()),
    );
    let request = p.canonical_request(&req("a")).expect("exec is cacheable");
    let live = crate::cache_key::request_cache_key(&request, 0, p.cache_salt(), None);
    let mut seen = std::collections::HashSet::new();
    for fingerprint in p.legacy_fingerprints() {
        let key = legacy_provider_key(&fingerprint, &req("a"), 0);
        assert_ne!(key, live, "a legacy key collided with the live one");
        assert!(
            seen.insert(key.0.clone()),
            "two legacy shapes produced one key, so a probe is wasted"
        );
    }
}

/// Both flavours are produced from one walk, and a file that resolves
/// contributes to both — the stat one is what a 0.3.1 store keyed on.
#[test]
fn a_resolvable_file_appears_in_both_flavours() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sut"), "#!/bin/sh\necho v1").unwrap();

    let (contents, stat) = legacy_programs(&command(&["./sut"]), Some(dir.path()));
    assert_eq!(contents.as_array().unwrap().len(), 1);
    assert_eq!(stat.as_array().unwrap().len(), 1);
    assert!(contents[0].get("content").is_some(), "{contents}");
    assert!(stat[0].get("mtime").is_some(), "{stat}");
}

/// A command naming nothing readable produces empty arrays. Such a provider
/// was not cacheable under the old rules, so there is nothing to adopt.
///
/// The program name is deliberately one no machine has. An earlier draft
/// used `docker`, which is on `PATH` on plenty of developer machines and on
/// most CI images — so the test asserted "resolves to nothing" about
/// something that resolves.
#[test]
fn an_unresolvable_command_yields_empty_programs() {
    let (contents, stat) = legacy_programs(
        &command(&["definitely-not-a-real-binary-xyz", "run", "img"]),
        None,
    );
    assert_eq!(contents, Json::Array(vec![]));
    assert_eq!(stat, Json::Array(vec![]));
}

// ── The frozen grader-verdict key ────────────────────────────────────────
//
// Same job as `golden_seven_part_key` above, for the other retired key
// space. Every relative assertion about grading identity now lives on the
// *live* path (the judge's request body), so what is left to pin here is the
// one thing no relative test can catch: the exact key a 0.4.x domarinn wrote.

fn rubric_assert(params: Option<crate::config::ParamMap>) -> Assert {
    Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::LlmRubric {
            value: "declines the task".into(),
            grader: None,
            threshold: None,
            params,
        },
    }
}

fn exec_assert(cache_salt: Option<&str>) -> Assert {
    Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::Exec {
            command: command(&["sh", "judge.sh"]),
            config: None,
            cache_salt: cache_salt.map(String::from),
        },
    }
}

fn judge() -> Grader {
    Grader {
        provider: ProviderKind::Anthropic {
            model: "claude-x".into(),
            base_url: Some("https://api.anthropic.com".into()),
            api_key_env: None,
            params: None,
            pricing: None,
            request: None,
            cache_salt: None,
        },
        template: None,
        verdict_mode: None,
        timeout_ms: None,
        include_tool_calls: None,
    }
}

fn graded_output() -> Output {
    Output::Text("I cannot help".into())
}

fn graded<'a>(output: &'a Output, vars: &'a Json) -> LegacyGraded<'a> {
    LegacyGraded {
        output,
        rubric: "declines the task",
        vars,
        test_id: "decline",
        test_tags: &[],
        provider_id: "p",
    }
}

/// The system prompt digest is a member, so the fixture pins the text it was
/// computed over rather than reaching for a const that is free to change.
const FIXTURE_SYSTEM_PROMPT: &str = "You are a strict evaluator.";

/// The exact key a 0.4.x domarinn wrote for one `llm-rubric` grading.
///
/// A magic constant on purpose: if it changes, every verdict a 0.4.x run
/// paid a judge for is unreachable, and no relative test would say so.
#[test]
fn golden_legacy_rubric_verdict_key() {
    let (output, vars) = (graded_output(), serde_json::json!({"country": "Norway"}));
    let assert = rubric_assert(None);
    let fingerprint =
        legacy_grading_fingerprint(&assert, Some(&judge()), FIXTURE_SYSTEM_PROMPT, None)
            .expect("an llm-rubric assert with a grader has a fingerprint");
    let payload =
        legacy_graded_payload(&assert, &graded(&output, &vars)).expect("llm-rubric is adopted");
    assert_eq!(
        legacy_grader_verdict_key(&fingerprint, &payload, 0).0,
        "sha256:c3e4acb40866f716122882ce212a092bfb4adaff4e380172fad1727ddc9cebf1"
    );
}

/// …and for one `exec` grading, whose fingerprint names the child and whose
/// payload carries the whole cell.
#[test]
fn golden_legacy_exec_verdict_key() {
    let (output, vars) = (graded_output(), serde_json::json!({"expected": "Paris"}));
    let assert = exec_assert(Some("v1"));
    let fingerprint =
        legacy_grading_fingerprint(&assert, None, FIXTURE_SYSTEM_PROMPT, None).expect("exec");
    let payload = legacy_graded_payload(&assert, &graded(&output, &vars)).expect("exec is adopted");
    assert_eq!(
        legacy_grader_verdict_key(&fingerprint, &payload, 0).0,
        "sha256:1eb5d685849b77e7b0dc9fdbd50a97bd0edff536d69862a872e5dbf618d40dc1"
    );
}

/// The two frozen halves, pinned as shapes rather than only as a hash, so a
/// failure names the member that moved instead of only the digest.
#[test]
fn the_frozen_halves_keep_their_member_names() {
    let (output, vars) = (graded_output(), serde_json::json!({}));
    let assert = rubric_assert(None);
    let fp =
        legacy_grading_fingerprint(&assert, Some(&judge()), FIXTURE_SYSTEM_PROMPT, None).unwrap();
    for member in [
        "assert",
        "provider",
        "template",
        "template_digest",
        "verdict_mode",
        "assert_params",
        "system_prompt",
    ] {
        assert!(fp.get(member).is_some(), "{member} left the fingerprint");
    }
    assert_eq!(fp["template_digest"], Json::Null, "no template, no digest");

    let exec = exec_assert(None);
    let payload = legacy_graded_payload(&exec, &graded(&output, &vars)).unwrap();
    for member in ["assert", "config", "output", "vars", "test", "provider"] {
        assert!(payload.get(member).is_some(), "{member} left the payload");
    }
}

/// A `grader.template`'s *bytes* were in the ≤0.4.x key, so a store keyed
/// with one prompt is not adoptable after that file is edited — which is the
/// property the digest existed for, preserved by the frozen copy.
#[test]
fn the_template_digest_still_reads_the_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("judge.md"), "Be lenient. {{rubric}}").unwrap();
    let mut g = judge();
    g.template = Some("file://judge.md".into());
    let assert = rubric_assert(None);
    let fingerprint = |base| {
        legacy_grading_fingerprint(&assert, Some(&g), FIXTURE_SYSTEM_PROMPT, Some(base)).unwrap()
    };

    let before = fingerprint(dir.path());
    assert!(before["template_digest"].is_string(), "{before}");
    std::fs::write(dir.path().join("judge.md"), "Be strict. {{rubric}}").unwrap();
    assert_ne!(before, fingerprint(dir.path()));

    // Unreadable digests to null rather than failing: a fingerprint's job is
    // only to move when the inputs move.
    assert_eq!(
        fingerprint(Path::new("/definitely/not/a/directory"))["template_digest"],
        Json::Null
    );
}

/// `similar` is not adopted, and the payload half is where that is decided —
/// so `zip`ping the two halves yields `None` and no probe is ever issued.
#[test]
fn a_similar_verdict_has_nothing_to_adopt() {
    let (output, vars) = (graded_output(), serde_json::json!({}));
    let assert = Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::Similar {
            value: crate::val::Val::Tpl(serde_json::json!("hello")),
            threshold: None,
        },
    };
    assert!(legacy_graded_payload(&assert, &graded(&output, &vars)).is_none());
    assert!(
        legacy_grading_fingerprint(&assert, Some(&judge()), FIXTURE_SYSTEM_PROMPT, None).is_none()
    );
}

/// The same guard the provider half carries, for the other key space: a
/// legacy verdict key must never equal the live request key for the same
/// grading call, or the probe would re-read the key that just missed.
///
/// Structural rather than probabilistic — `request` is not a member of the
/// frozen object, and `kind`/`fingerprint`/`graded` are not members of the
/// live one — and fed the most adversarial input available: the legacy key's
/// own parts, handed to the live key as the request.
#[test]
fn no_legacy_grader_key_equals_the_live_request_key() {
    let (output, vars) = (graded_output(), serde_json::json!({"country": "Norway"}));
    for assert in [rubric_assert(None), exec_assert(Some("v1"))] {
        let fingerprint =
            legacy_grading_fingerprint(&assert, Some(&judge()), FIXTURE_SYSTEM_PROMPT, None)
                .unwrap();
        let payload = legacy_graded_payload(&assert, &graded(&output, &vars)).unwrap();
        let legacy = legacy_grader_verdict_key(&fingerprint, &payload, 0);
        let parts = serde_json::json!({
            "kind": "grader-verdict",
            "fingerprint": fingerprint,
            "graded": payload,
            "repeat": 0,
        });
        for salt in [None, Some("v1")] {
            assert_ne!(
                crate::cache_key::request_cache_key(&parts, 0, None, salt),
                legacy,
                "a legacy grader key collided with a live request key"
            );
        }
    }
}

/// The budget is spent per case and stops a pointless probe loop, but one
/// adoption buys unlimited further probing.
#[test]
fn the_probe_budget_stops_when_nothing_is_adopted() {
    let probe = MigrationProbe::new();
    for _ in 0..PROBE_BUDGET {
        assert!(probe.should_probe());
    }
    assert!(!probe.should_probe(), "budget must run out");

    let probe = MigrationProbe::new();
    assert!(probe.should_probe());
    probe.record_adoption();
    for _ in 0..(PROBE_BUDGET * 10) {
        assert!(probe.should_probe(), "an adoption lifts the budget");
    }
}

#[test]
fn a_disabled_probe_never_fires() {
    let probe = MigrationProbe::disabled();
    assert!(!probe.should_probe());
    assert!(!probe.adopted_any());
}
