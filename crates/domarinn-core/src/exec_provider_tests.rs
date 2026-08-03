//! Unit tests for [`super`] (the exec provider protocol: request shape,
//! response parsing, error surfacing). Split out of `exec_provider.rs` via
//! `#[path]` to keep that file under the repo's 1000-line source cap; this
//! is still its private child module (`use super::*`).

use super::*;
use crate::provider::TestMeta;
use serde_json::json;

/// See the matching test in `anthropic.rs`: the fingerprint feeds every
/// cache key, so an unconditional change invalidates every cached entry.
#[test]
fn fingerprint_is_stable_for_default_config() {
    let p = ExecProvider::new("p", vec!["./sut".into()], BTreeMap::new(), None, None, None);
    // The asserted string has changed three times, each invalidating every
    // exec entry in every store: when `program` was added to make exec
    // caching safe by default, when `env` joined it, and when `program` was
    // removed again because keying the local filesystem is what made an exec
    // entry unshareable in the first place. Any *further* change does the
    // same, so treat a failure here as a cache migration to plan rather than
    // a test to update — and see `crate::cache_migrate`, which is how the
    // last one was paid for rather than charged to every user.
    assert_eq!(
        crate::cache::canonical_json(&p.fingerprint()),
        r#"{"cache_salt":null,"command":["./sut"],"env":null,"type":"exec"}"#
    );
    assert!(p.cacheable(), "exec caches like every other provider kind");
}

/// The headline portability property, at the smallest scale that can show
/// it: the same command keys the same way from anywhere, whether or not the
/// program is even present. Before this, `base_dir` decided whether a
/// program was found, so a suite run from a repo root and the same suite run
/// from its own directory produced two different keys for one question.
#[test]
fn the_fingerprint_does_not_depend_on_where_the_program_lives() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sut"), "#!/bin/sh\necho v1").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("sut"), "#!/bin/sh\necho v1").unwrap();

    let command = vec!["./sut".to_string()];
    let fp = |base: Option<&std::path::Path>| {
        let p = ExecProvider::new("p", command.clone(), BTreeMap::new(), None, None, base);
        crate::cache::canonical_json(&p.fingerprint())
    };

    assert_eq!(fp(Some(dir.path())), fp(Some(elsewhere.path())));
    assert_eq!(
        fp(Some(dir.path())),
        fp(None),
        "a program that cannot be found must key exactly like one that can"
    );
    assert!(!fp(Some(dir.path())).contains(&dir.path().display().to_string()));
}

/// A tool installed on `PATH` is the most ordinary exec provider there is,
/// and the one that used to poison a shared cache hardest: its contents were
/// hashed, so a NixOS store path and a Debian `/usr/bin` never agreed even
/// when the suite, the question and the answer were identical.
#[test]
fn a_path_resolved_program_does_not_enter_the_key() {
    let p = ExecProvider::new("p", vec!["sh".into()], BTreeMap::new(), None, None, None);
    assert!(p.cacheable());
    let fp = crate::cache::canonical_json(&p.fingerprint());
    assert_eq!(
        fp,
        r#"{"cache_salt":null,"command":["sh"],"env":null,"type":"exec"}"#
    );
    // It is still *reported*, so a rebuild can be warned about.
    assert!(p.program_digest().is_some(), "sh resolves on PATH");
}

/// Two backends behind one wrapper script is a normal A/B shape. Sharing a
/// key would make the second column replay the first's answers — fabricating
/// exactly the comparison the run exists to make.
#[test]
fn providers_differing_only_in_env_do_not_share_a_key() {
    let of = |endpoint: &str| {
        ExecProvider::new(
            "p",
            vec!["sh".into()],
            BTreeMap::from([("MODEL_ENDPOINT".to_string(), endpoint.to_string())]),
            None,
            None,
            None,
        )
    };
    let (a, b) = (of("http://a"), of("http://b"));
    assert_ne!(
        crate::cache::canonical_json(&a.fingerprint()),
        crate::cache::canonical_json(&b.fingerprint())
    );
    // A digest, never the map: a fingerprint is persisted into the cache
    // entry and `env` is where an exec provider's credentials live.
    let fp = crate::cache::canonical_json(&a.fingerprint());
    assert!(!fp.contains("http://a"), "{fp}");
    assert!(!fp.contains("MODEL_ENDPOINT"), "{fp}");
}

/// A rebuild does **not** bust the key, and that is the deliberate trade.
///
/// This test asserted the opposite for two releases. Busting on a rebuild
/// meant the key described the local filesystem, which is why no two
/// machines — and no CI job that compiled its own provider — could ever
/// share an entry. `cache_salt` is now how a suite says "different build",
/// and the digest below is how a run says "you may have meant to".
#[test]
fn a_rebuild_moves_the_digest_but_not_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let prog = dir.path().join("sut");
    let command = vec![prog.to_string_lossy().to_string()];
    let built = |contents: &str| {
        std::fs::write(&prog, contents).unwrap();
        let p = ExecProvider::new("p", command.clone(), BTreeMap::new(), None, None, None);
        (
            crate::cache::canonical_json(&p.fingerprint()),
            p.program_digest().map(str::to_string),
        )
    };

    let (before_key, before_digest) = built("#!/bin/sh\necho v1");
    let (after_key, after_digest) = built("#!/bin/sh\necho v2 and then some more");

    assert_eq!(before_key, after_key, "a rebuild must not re-key the cache");
    assert_ne!(
        before_digest, after_digest,
        "…but it must be visible, so the hit can be warned about"
    );
    assert!(before_digest.is_some_and(|d| d.starts_with("blake3:")));
}

/// Bumping `cache_salt` is the supported way to throw the old answers away.
#[test]
fn a_salt_is_what_separates_two_builds() {
    let provider = |salt: &str| {
        ExecProvider::new(
            "p",
            vec!["./sut".into()],
            BTreeMap::new(),
            None,
            Some(salt.to_string()),
            None,
        )
    };
    let of = |salt: &str| crate::cache::canonical_json(&provider(salt).fingerprint());
    assert_ne!(of("abc1234"), of("def5678"));
    // A key member, never a request member: publishing it separates the
    // builds, and sending it would change the child's input.
    let p = provider("abc1234");
    assert_eq!(p.cache_salt(), Some("abc1234"));
    let canonical = crate::cache::canonical_json(&p.canonical_request(&request("hi")).unwrap());
    assert!(!canonical.contains("abc1234"), "{canonical}");
}

/// Nothing the filesystem knows may reach the key — not contents, not
/// `mtime`, not size, not a path. Asserted as one property rather than one
/// test per attribute, because the rule is the point and the individual
/// attributes are just the ways it has been broken so far.
#[test]
fn no_property_of_the_filesystem_reaches_the_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let prog = dir.path().join("sut");
    let command = vec![prog.to_string_lossy().to_string()];
    let fingerprint = || {
        let p = ExecProvider::new("p", command.clone(), BTreeMap::new(), None, None, None);
        crate::cache::canonical_json(&p.fingerprint())
    };

    // Absent.
    let absent = fingerprint();
    // Present, one size.
    std::fs::write(&prog, "#!/bin/sh\necho v1").unwrap();
    assert_eq!(absent, fingerprint(), "existence must not key");
    // Present, different size and contents.
    std::fs::write(&prog, "#!/bin/sh\necho something considerably longer").unwrap();
    assert_eq!(absent, fingerprint(), "size and contents must not key");
    // `mtime` needs no leg of its own: rewriting the file above moved it,
    // and the key did not. It is subsumed rather than skipped — and the
    // portability suite exercises it explicitly, with a real backdated
    // stamp, in `tests/cache_portability.rs`.
}

#[test]
fn request_preview_is_the_document_written_to_stdin() {
    let p = ExecProvider::new(
        "e",
        vec!["./sut".into(), "--mode".into(), "eval".into()],
        // A credential in the provider env — must not reach the preview,
        // which is persisted into a shareable run document.
        BTreeMap::from([("SUT_TOKEN".to_string(), "secret".to_string())]),
        None,
        None,
        None,
    );
    let req = request("hello");

    let preview = p.request_preview(&req).unwrap();
    assert_eq!(preview["transport"], json!("exec"));
    assert_eq!(preview["command"], json!("./sut"));
    assert_eq!(preview["args"], json!(["--mode", "eval"]));
    // Byte-identical to what `call` serializes: `Envelope` carries only a
    // protocol version and a kind, nothing per-invocation.
    assert_eq!(
        preview["stdin"],
        serde_json::to_value(protocol_request(&req)).unwrap()
    );
    assert_eq!(preview["stdin"]["vars"]["user_input"], json!("hello"));
    assert!(!preview.to_string().contains("secret"));
}

/// The documented exception to "the keyed request is what is sent": the
/// `test` block is correlation metadata, like a request id, so two cases
/// with identical vars keep sharing an entry. The child still receives it —
/// the preview is what proves that.
#[test]
fn the_keyed_stdin_drops_the_test_block_but_keeps_the_protocol_envelope() {
    let p = ExecProvider::new("e", vec!["./sut".into()], BTreeMap::new(), None, None, None);
    let req = request("hello");

    let canonical = p.canonical_request(&req).unwrap();
    let stdin = &canonical["stdin"];
    assert!(stdin.get("test").is_none(), "{stdin}");
    assert_eq!(stdin["domarinn"]["kind"], json!("provider"));
    assert!(stdin["domarinn"]["protocol"].is_number());
    assert_eq!(stdin["vars"]["user_input"], json!("hello"));
    assert_eq!(canonical["command"], json!("./sut"));

    assert_eq!(
        p.request_preview(&req).unwrap()["stdin"]["test"]["id"],
        json!("t"),
        "the real call still sends it"
    );
    assert!(p
        .canonical_request(&request("x"))
        .is_some_and(|c| c.get("env_digest").is_none()));
}

/// `env` selects the thing that answers (`MODEL_ENDPOINT: http://a` against
/// `…/b`), so it keys — as a digest, never as values: a canonical request is
/// persisted into every cache entry and entries travel to shared stores.
#[test]
fn the_env_digest_keys_without_publishing_the_environment() {
    let p = ExecProvider::new(
        "e",
        vec!["./sut".into()],
        BTreeMap::from([("SUT_TOKEN".to_string(), "SENTINEL-SECRET".to_string())]),
        None,
        None,
        None,
    );
    let canonical = p.canonical_request(&request("hi")).unwrap();
    assert_eq!(canonical["env_digest"], json!(p.env_digest.clone()));
    assert!(canonical["env_digest"]
        .as_str()
        .unwrap()
        .starts_with("blake3:"));
    let serialized = crate::cache::canonical_json(&canonical);
    assert!(!serialized.contains("SENTINEL-SECRET"), "{serialized}");
    assert!(!serialized.contains("SUT_TOKEN"), "{serialized}");
}

#[test]
fn request_preview_is_absent_for_an_empty_command() {
    let p = ExecProvider::new("e", Vec::new(), BTreeMap::new(), None, None, None);
    assert!(p.request_preview(&request("hi")).is_none());
}

fn request(user_input: &str) -> ProviderRequest {
    let mut vars = BTreeMap::new();
    vars.insert("user_input".to_string(), Json::String(user_input.into()));
    ProviderRequest {
        tools: Vec::new(),
        prompt: None,
        vars,
        params: serde_json::Map::new(),
        test: TestMeta {
            id: "t".into(),
            tags: vec![],
        },
        case_salt: None,
    }
}

// A tiny inline provider script using `jq`-free shell: read stdin, emit a
// fixed protocol response. We use `cat`-style echo via a python one-liner if
// available; otherwise skip. To stay dependency-free we use `sh -c` echoing
// a valid response regardless of input.
fn echo_provider() -> ExecProvider {
    ExecProvider::new(
        "p",
        vec![
            "sh".into(),
            "-c".into(),
            // ignore stdin, print a protocol response
            "cat >/dev/null; printf '{\"output\":\"hello\"}'".into(),
        ],
        BTreeMap::new(),
        Some(5000),
        Some("salt".into()),
        None,
    )
}

#[tokio::test]
async fn exec_provider_returns_output() {
    let provider = echo_provider();
    let resp = provider
        .call(&request("x"), &CallCtx::default())
        .await
        .unwrap();
    assert_eq!(resp.output, Output::Text("hello".into()));
}

/// The per-case salt keys the cache entry; it must never reach the child's
/// stdin. Proved on the wire rather than structurally, so that "completing"
/// the `ProviderReq` mapping later fails loudly.
#[tokio::test]
async fn case_salt_is_not_sent_to_the_child() {
    // Echo the received request back as the `output` value.
    let provider = ExecProvider::new(
        "p",
        vec![
            "sh".into(),
            "-c".into(),
            r#"printf '{"output":'; cat; printf '}'"#.into(),
        ],
        BTreeMap::new(),
        Some(5000),
        Some("salt".into()),
        None,
    );
    let mut req = request("x");
    req.case_salt = Some("SENTINEL-DIGEST".into());
    let resp = provider.call(&req, &CallCtx::default()).await.unwrap();
    let seen = format!("{:?}", resp.output);
    assert!(
        seen.contains("user_input"),
        "sanity: the child should have echoed the request back, got {seen}"
    );
    assert!(
        !seen.contains("SENTINEL-DIGEST"),
        "case_salt leaked into the child's request: {seen}"
    );
}

#[tokio::test]
async fn exec_providers_are_cached_by_default() {
    // This used to require a `cache_salt`. Everything that can be cached is
    // now cached by default; `program` in the fingerprint is what makes that
    // safe, and the salt is the escape hatch rather than the entry ticket.
    let with_salt = ExecProvider::new(
        "p",
        vec!["true".into()],
        BTreeMap::new(),
        None,
        Some("v1".into()),
        None,
    );
    let without = ExecProvider::new("p", vec!["true".into()], BTreeMap::new(), None, None, None);
    assert!(with_salt.cacheable());
    assert!(without.cacheable());
    // The salt still separates them.
    assert_ne!(
        crate::cache::canonical_json(&with_salt.fingerprint()),
        crate::cache::canonical_json(&without.fingerprint())
    );
}

/// A command that resolves to nothing on disk caches like any other.
///
/// This asserted the opposite — `docker run …` was refused a cache, because
/// argv alone "does not move when the program is rebuilt". Every command is
/// keyed on argv alone now, deliberately, so the special case has nothing
/// left to be special about. What it loses along the way is real and worth
/// naming: an unresolvable command has no `program_digest` either, so a
/// rebuild behind `docker run` cannot even be *warned* about. `cache_salt`
/// is the only signal available there, which is exactly what it was before.
#[tokio::test]
async fn a_command_that_resolves_to_nothing_is_still_cached() {
    let unresolvable = vec!["definitely-not-a-real-binary-xyz".to_string()];
    let bare = ExecProvider::new("p", unresolvable.clone(), BTreeMap::new(), None, None, None);
    assert!(bare.cacheable());
    assert!(
        bare.program_digest().is_none(),
        "nothing on disk to digest, so nothing to report"
    );

    let salted = ExecProvider::new(
        "p",
        unresolvable,
        BTreeMap::new(),
        None,
        Some("v1".into()),
        None,
    );
    assert_ne!(
        crate::cache::canonical_json(&bare.fingerprint()),
        crate::cache::canonical_json(&salted.fingerprint()),
        "the salt is the only lever such a command has"
    );
}

#[tokio::test]
async fn provider_error_from_child_is_surfaced() {
    let provider = ExecProvider::new(
        "p",
        vec![
            "sh".into(),
            "-c".into(),
            "cat >/dev/null; printf '{\"output\":\"\",\"error\":{\"message\":\"boom\",\"retriable\":true}}'".into(),
        ],
        BTreeMap::new(),
        Some(5000),
        None,
        None,
    );
    let err = provider
        .call(&request("x"), &CallCtx::default())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Retriable { .. }));
}

/// Parse a response document the way a child would have written it.
fn parse(body: serde_json::Value) -> Result<ProviderResponse, ProviderError> {
    parse_response(body)
}

/// The back-compat floor for every field added to protocol 1: a child that
/// sets only `output` must behave exactly as it did before they existed.
#[test]
fn a_response_with_none_of_the_new_fields_is_unchanged() {
    let resp = parse(serde_json::json!({"output": "hi"})).unwrap();
    assert_eq!(resp.output, Output::Text("hi".into()));
    assert_eq!(resp.stop_reason, None);
    assert_eq!(resp.empty_reason, None);
    assert_eq!(resp.reasoning, None);
    assert!(resp.usage.is_none());
}

/// The gap this closed: an exec child that knows the model refused had no
/// way to say so, so the cell scored 0 against every assertion as if the
/// prompt were bad.
#[test]
fn a_child_reported_empty_reason_reaches_the_response() {
    let resp = parse(serde_json::json!({"output": "", "empty_reason": "refusal"})).unwrap();
    assert_eq!(
        resp.empty_reason.as_ref().map(|r| r.as_str()),
        Some("refusal")
    );
}

/// An empty string is no claim at all, not a claim of an unnamed reason.
/// Letting it through would produce the one `CaseResult` shape the rest of
/// the pipeline disagrees about: `''` is the storage layer's "known: not
/// empty" sentinel, so the CLI would count a case the server does not.
#[test]
fn an_empty_string_empty_reason_is_no_claim() {
    let resp = parse(serde_json::json!({"output": "hi", "empty_reason": ""})).unwrap();
    assert_eq!(resp.empty_reason, None);
}

/// Open set: a reason this build has never heard of is carried verbatim,
/// never rejected. Rejecting it would turn a diagnosis into a parse failure.
#[test]
fn an_unknown_empty_reason_is_carried_verbatim() {
    let resp = parse(serde_json::json!({"output": "", "empty_reason": "invented_later"})).unwrap();
    assert_eq!(
        resp.empty_reason.as_ref().map(|r| r.as_str()),
        Some("invented_later")
    );
}

#[test]
fn a_blank_output_with_only_a_stop_reason_derives_one() {
    let resp = parse(serde_json::json!({"output": "", "stop_reason": "max_tokens"})).unwrap();
    assert_eq!(
        resp.empty_reason.as_ref().map(|r| r.as_str()),
        Some(crate::empty::EmptyReason::TRUNCATED)
    );
    assert_eq!(resp.stop_reason.as_deref(), Some("max_tokens"));
}

/// A model that hit `max_tokens` *after* answering was not truncated into
/// silence. Labelling that "truncated" sends the reader after the wrong fix,
/// so derivation is gated on the output actually being blank.
#[test]
fn a_non_blank_output_never_derives_an_empty_reason() {
    let resp =
        parse(serde_json::json!({"output": "a real answer", "stop_reason": "max_tokens"})).unwrap();
    assert_eq!(resp.empty_reason, None);
    assert_eq!(resp.stop_reason.as_deref(), Some("max_tokens"));
}

#[test]
fn child_reported_cache_tokens_reach_token_usage() {
    let resp = parse(serde_json::json!({
        "output": "hi",
        "usage": {
            "input_tokens": 5, "output_tokens": 2,
            "cache_read_tokens": 100, "cache_write_tokens": 40,
            "cache_write_1h_tokens": 10
        }
    }))
    .unwrap();
    let usage = resp.usage.unwrap();
    assert_eq!(usage.cache_read_tokens, Some(100));
    assert_eq!(usage.cache_write_tokens, Some(40));
    assert_eq!(usage.cache_write_1h_tokens, Some(10));
    // `total()` is the whole exchange — the cached span is prompt that was
    // sent, so a `tokens:` budget counts it. `billable_total()` adds the
    // cache *write*, which is spend rather than prompt.
    assert_eq!(usage.total(), 5 + 2 + 100);
    assert_eq!(usage.billable_total(), 5 + 2 + 100 + 40);
}

#[test]
fn a_child_error_keeps_its_structured_details() {
    let err = parse(serde_json::json!({
        "output": "",
        "error": {
            "message": "upstream refused",
            "retriable": false,
            "details": {"status": 403, "model": "m-1"}
        }
    }))
    .unwrap_err();
    assert_eq!(
        err.details(),
        Some(&serde_json::json!({"status": 403, "model": "m-1"}))
    );
}

/// The reported bug, verbatim: `parse_response` returned before `metadata`
/// was read, so a child that sent diagnostics alongside an error had them
/// silently discarded. The fallback fixes it for children that never change.
#[test]
fn a_child_error_without_details_falls_back_to_metadata() {
    let err = parse(serde_json::json!({
        "output": "",
        "error": {"message": "boom", "retriable": false},
        "metadata": {"attempt": 3}
    }))
    .unwrap_err();
    assert_eq!(err.details(), Some(&serde_json::json!({"attempt": 3})));
}

/// Every exec failure used to be `exec_failed`, so a child that knew its
/// credential was rejected could not say so and the error-class vocabulary
/// was blind to the one provider type most people extend with.
#[test]
fn a_child_can_name_its_own_error_class() {
    let err = parse(serde_json::json!({
        "output": "",
        "error": {"message": "401", "retriable": false, "class": "provider_auth"}
    }))
    .unwrap_err();
    assert_eq!(err.class().as_str(), ErrorClass::PROVIDER_AUTH);
}

#[test]
fn an_unnamed_class_still_defaults_to_exec_failed() {
    let err = parse(serde_json::json!({
        "output": "", "error": {"message": "boom", "retriable": false}
    }))
    .unwrap_err();
    assert_eq!(err.class().as_str(), ErrorClass::EXEC_FAILED);
}

/// All three native providers parse a `Retry-After`; the exec child was the
/// only one that had to swallow it.
#[test]
fn a_child_can_supply_a_retry_after() {
    let err = parse(serde_json::json!({
        "output": "",
        "error": {"message": "slow down", "retriable": true, "retry_after_ms": 2500}
    }))
    .unwrap_err();
    match err {
        ProviderError::Retriable { retry_after, .. } => {
            assert_eq!(retry_after, Some(Duration::from_millis(2500)));
        }
        other => panic!("expected a retriable error, got {other:?}"),
    }
}
