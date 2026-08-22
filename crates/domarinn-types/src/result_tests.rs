//! Tests for [`super`]'s result types — keys, statuses, serialization
//! round-trips. Split out of `result.rs` the same way `change_tests.rs`
//! is, to keep the type definitions under the file-length ratchet.

use super::*;
#[test]
fn case_key_is_deterministic_and_repeat_sensitive() {
    let base = CellKey {
        provider_id: "p".into(),
        prompt_id: Some("prompt".into()),
        test_id: "t".into(),
        repeat: 0,
    };
    let mut r1 = base.clone();
    r1.repeat = 1;
    assert_eq!(base.case_key(), base.case_key());
    assert_ne!(base.case_key(), r1.case_key());
    assert_eq!(base.case_key().as_str().len(), 16);
}

#[test]
fn case_key_distinguishes_prompt_presence() {
    let with = CellKey {
        provider_id: "p".into(),
        prompt_id: Some("x".into()),
        test_id: "t".into(),
        repeat: 0,
    };
    let without = CellKey {
        prompt_id: None,
        ..with.clone()
    };
    assert_ne!(with.case_key(), without.case_key());
}

#[test]
fn v1_case_result_deserializes_with_absent_v2_fields_and_re_serializes_byte_stable() {
    // A v1 result document has none of the added optional keys. It must
    // deserialize with each defaulting to its empty/`None` value, and —
    // because they carry `skip_serializing_if` — re-serialize without
    // emitting any of them, so a stored-then-reloaded v1 document is
    // byte-identical (the server's content-hash idempotency depends on
    // absent fields staying absent). This guards `prompt`/`stop_reason`/`raw`
    // and the later-added `vars` (case level) and `criteria` (assert level).
    let v1 = r#"{
        "cell": {"provider_id": "p", "test_id": "t"},
        "case_key": "0011223344556677",
        "status": "pass",
        "score": 1.0,
        "output": "hello",
        "asserts": [
            {"kind": "contains", "status": "pass", "score": 1.0,
             "weight": 1.0, "reason": "ok", "cached": false}
        ],
        "latency_ms": 12
    }"#;
    let case: CaseResult = serde_json::from_str(v1).unwrap();
    assert!(case.prompt.is_none());
    assert!(case.stop_reason.is_none());
    assert!(case.raw.is_none());
    assert!(case.vars.is_empty());
    assert!(case.asserts[0].criteria.is_none());
    assert!(case.model.is_none());
    assert!(case.error_details.is_none());

    let reserialized = serde_json::to_string(&case).unwrap();
    assert!(!reserialized.contains("prompt"));
    assert!(!reserialized.contains("stop_reason"));
    assert!(!reserialized.contains("\"raw\""));
    assert!(!reserialized.contains("vars"));
    assert!(!reserialized.contains("criteria"));
    // The later-added digest and classification fields. `prompt_digest`
    // would be caught by the `prompt` assertion above only by accident;
    // the other three had no guard at all, so dropping a
    // `skip_serializing_if` from any of them used to pass silently — and
    // every historical run's content hash would shift on re-ingest.
    assert!(!reserialized.contains("prompt_digest"));
    assert!(!reserialized.contains("provider_digest"));
    assert!(!reserialized.contains("assert_digest"));
    assert!(!reserialized.contains("error_class"));
    // The reported model and structured error detail. Same hazard: a
    // `skip_serializing_if` dropped from either would make every stored
    // run grow a `null` on re-serialization, moving the content hash the
    // server uses for ingest idempotency.
    assert!(!reserialized.contains("\"model\""));
    assert!(!reserialized.contains("error_details"));
    // The per-assertion grading cost. It sits inside every assert of every
    // case, so it is the densest of these keys: emitting it as `null` would
    // move the content hash of every stored run that has any assertions.
    assert!(case.asserts[0].cost_usd.is_none());
    assert!(!reserialized.contains("cost_usd"));
    // Tool calls, same hazard: a `Vec` without `skip_serializing_if` would
    // add `"tool_calls":[]` to every stored case ever written.
    assert!(case.tool_calls.is_empty());
    assert!(!reserialized.contains("tool_calls"));
    // The expected-failure annotation's reason. Only annotated cases carry
    // it; every historical case must stay byte-identical.
    assert!(case.expect_fail_reason.is_none());
    assert!(!reserialized.contains("expect_fail"));
}

/// The run-level counterpart of the guard above.
///
/// The server's ingest idempotency key is `sha256(canonical_json(run))` over
/// the *whole* document, so a stored run that never carried `origin` (or
/// `git`, `ci`, `share_url`) must not grow the key on a load/store round
/// trip — that would change its content hash and turn an idempotent
/// re-upload into a 409 Conflict.
#[test]
fn a_run_without_optional_provenance_does_not_grow_it_on_re_serialization() {
    let stored = r#"{
        "schema_version": 2,
        "run_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "started_at": "2026-01-01T00:00:00Z",
        "finished_at": "2026-01-01T00:00:01Z",
        "config_digest": "blake3:abc",
        "config_snapshot": {},
        "cases": [],
        "summary": {"total": 0, "passed": 0, "failed": 0, "errored": 0, "skipped": 0}
    }"#;
    let run: RunResult = serde_json::from_str(stored).unwrap();
    assert!(run.origin.is_none());
    assert!(run.digests.is_none());
    assert!(run.git.is_none());
    assert!(run.ci.is_none());
    assert!(run.share_url.is_none());
    assert!(run.composite.is_none());

    let reserialized = serde_json::to_string(&run).unwrap();
    assert!(!reserialized.contains("origin"));
    assert!(!reserialized.contains("\"git\""));
    assert!(!reserialized.contains("\"ci\""));
    assert!(!reserialized.contains("share_url"));
    assert!(!reserialized.contains("digests"));
    // The cost/cache counters added alongside. Unlike the older bare-
    // `default` fields on RunSummary, these must stay absent at zero — or
    // every historical run grows three keys on re-serialization and the
    // content hash the server ingests on moves.
    assert!(!reserialized.contains("cache_read_tokens"));
    assert!(!reserialized.contains("cache_write_tokens"));
    assert!(!reserialized.contains("cache_savings_usd"));
    assert!(!reserialized.contains("grader_cost_usd"));
    // The expected-failure counters, added with the same contract.
    assert!(!reserialized.contains("xfailed"));
    assert!(!reserialized.contains("xpassed"));
    // The synthetic branch-baseline marker: only ever set on a merged
    // baseline document built in memory, so a stored run must never grow it.
    assert!(!reserialized.contains("composite"));
}

/// Same 409 fence, for the empty-output tally: a run where nothing came
/// back empty must not grow an `empty_counts: {}` key it never had.
#[test]
fn a_run_without_empty_cases_does_not_serialize_empty_counts() {
    let summary = RunSummary {
        total: 1,
        passed: 1,
        ..Default::default()
    };
    assert!(summary.empty_counts.is_empty());

    let value = serde_json::to_value(&summary).unwrap();
    assert!(
        value.get("empty_counts").is_none(),
        "empty_counts must be absent when empty: {value}"
    );
}

#[test]
fn case_status_as_str_matches_serde_and_round_trips_via_from_str() {
    for status in [
        CaseStatus::Pass,
        CaseStatus::Fail,
        CaseStatus::Error,
        CaseStatus::Skip,
        CaseStatus::XFail,
        CaseStatus::XPass,
    ] {
        let serde_str = serde_json::to_value(status).unwrap();
        assert_eq!(serde_str, serde_json::json!(status.as_str()));
        assert_eq!(status.as_str().parse::<CaseStatus>().unwrap(), status);
    }
    assert!("bogus".parse::<CaseStatus>().is_err());
}

/// The enum is `rename_all = "snake_case"`, under which `XFail` would
/// serialize as `"x_fail"` — a wire string no reader expects. The variants
/// carry explicit renames; this pins the exact strings.
#[test]
fn expected_failure_statuses_serialize_without_a_snake_case_underscore() {
    assert_eq!(
        serde_json::to_string(&CaseStatus::XFail).unwrap(),
        "\"xfail\""
    );
    assert_eq!(
        serde_json::to_string(&CaseStatus::XPass).unwrap(),
        "\"xpass\""
    );
}

/// Same 409 fence as the guards above, for the expected-failure counters:
/// a run with no `expect_fail` cases must not grow `xfailed`/`xpassed`
/// keys on re-serialization, and a summary that has them must emit them.
#[test]
fn expected_failure_counters_are_absent_at_zero_and_present_when_set() {
    let quiet = RunSummary {
        total: 1,
        passed: 1,
        ..Default::default()
    };
    let value = serde_json::to_value(&quiet).unwrap();
    assert!(
        value.get("xfailed").is_none(),
        "xfailed must skip at 0: {value}"
    );
    assert!(
        value.get("xpassed").is_none(),
        "xpassed must skip at 0: {value}"
    );

    let marked = RunSummary {
        total: 2,
        xfailed: 1,
        xpassed: 1,
        ..Default::default()
    };
    let value = serde_json::to_value(&marked).unwrap();
    assert_eq!(value["xfailed"], 1);
    assert_eq!(value["xpassed"], 1);
}

/// The smallest document the current `CaseResult` accepts.
fn minimal_case() -> &'static str {
    r#"{
        "cell": {"provider_id": "p", "test_id": "t"},
        "case_key": "0011223344556677",
        "status": "pass",
        "score": 1.0,
        "output": "hello",
        "asserts": [],
        "latency_ms": 12
    }"#
}

/// The compatibility contract that makes `cache_key` additive rather than a
/// schema-version bump: a document written before the field existed still
/// parses, and one written by a client that has no key omits it entirely.
///
/// This holds because nothing in this crate uses `deny_unknown_fields` —
/// which is also why `docs/reference/server.md`'s claim that an unknown
/// field fails ingest validation is wrong.
#[test]
fn a_case_recorded_before_cache_key_existed_still_parses() {
    let case: CaseResult = serde_json::from_str(minimal_case()).expect("parses");
    assert!(case.cache_key.is_none());
}

#[test]
fn an_unknown_field_does_not_fail_a_case_document() {
    let mut doc: serde_json::Value = serde_json::from_str(minimal_case()).unwrap();
    doc["a_field_from_a_newer_domarinn"] = serde_json::json!(42);
    assert!(serde_json::from_value::<CaseResult>(doc).is_ok());
}

#[test]
fn a_case_with_no_key_omits_it_from_the_wire() {
    let case: CaseResult = serde_json::from_str(minimal_case()).unwrap();
    let wire = serde_json::to_value(&case).unwrap();
    assert!(wire.get("cache_key").is_none(), "{wire}");
}

#[test]
fn a_recorded_cache_key_round_trips() {
    let key = "sha256:".to_string() + &"ab".repeat(32);
    let mut doc: serde_json::Value = serde_json::from_str(minimal_case()).unwrap();
    doc["cache_key"] = serde_json::json!(key);
    let case: CaseResult = serde_json::from_value(doc).unwrap();
    assert_eq!(case.cache_key.as_deref(), Some(key.as_str()));
    assert_eq!(
        serde_json::to_value(&case).unwrap()["cache_key"],
        serde_json::json!(key)
    );
}
