//! Unit tests for [`super`] (the default grader). Split out of `grader.rs`
//! via `#[path]` to keep that file under the repo's 1000-line source cap;
//! this is still the grader's private child module (`use super::*`).

use super::*;

/// A judge with no `request:` block — the shape every one of these tests means
/// unless it says otherwise.
fn judge_default() -> crate::request_cfg::ResolvedRequest {
    let (path, auth) = Judge::Anthropic.defaults();
    crate::request_cfg::ResolvedRequest::vendor_default(path, auth)
}
use crate::config::Grader;
use crate::config::ParamMap;
use crate::error_class::ErrorClass;
use crate::errors::Classify;
use crate::template::TemplateEngine;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Identity for a cell under test. The values are arbitrary; what matters
/// is that they are no longer empty strings on the wire.
///
/// `cache: None` — these exercise the grading itself, so every call goes live.
/// What the cache does with those calls is `tests/grader_request_cache.rs`.
fn grade_ctx<'a>(vars: &'a Json, engine: &'a TemplateEngine) -> GradeCtx<'a> {
    grade_ctx_with_calls(vars, engine, &[])
}

/// The same cell, with tool calls attached. Separate from [`grade_ctx`] so the
/// dozen tests that grade prose keep reading as prose.
fn grade_ctx_with_calls<'a>(
    vars: &'a Json,
    engine: &'a TemplateEngine,
    tool_calls: &'a [crate::result::ToolCall],
) -> GradeCtx<'a> {
    GradeCtx {
        vars,
        engine,
        working_dir: None,
        provider_id: "p",
        test_id: "t",
        test_tags: &[],
        tool_calls,
        cache: None,
    }
}

/// One call with a vendor id on it, so every test that asserts the id is
/// *absent* is asserting against something that was actually there.
fn one_tool_call() -> Vec<crate::result::ToolCall> {
    vec![crate::result::ToolCall {
        id: Some("toolu_01ABCDEF".into()),
        name: "get_weather".into(),
        arguments: json!({"city": "Reykjavik"}),
    }]
}

fn anthropic_grader(uri: &str) -> Grader {
    Grader {
        provider: ProviderKind::Anthropic {
            model: "claude-x".into(),
            base_url: Some(uri.to_string()),
            api_key_env: Some("GRADER_TEST_KEY".into()),
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

fn rubric_assert() -> Assert {
    Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::LlmRubric {
            value: "declines the task".into(),
            grader: None,
            threshold: None,
            params: None,
        },
    }
}

#[tokio::test]
async fn anthropic_tool_use_verdict_passes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stop_reason": "tool_use",
            "content": [{
                "type": "tool_use", "name": "submit_verdict",
                "input": {"reasoning": "it declines", "pass": true, "score": 0.9}
            }]
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
    let outcome = grader
        .grade(
            &rubric_assert(),
            &Output::Text("I cannot help with that".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap()
        .verdict
        .to_outcome(None);
    assert!(outcome.passed);
    assert!((outcome.score - 0.9).abs() < 1e-9);
}

/// The judge's own bill. `pricing:` on a `grader.provider` used to be parsed
/// and ignored, so the model doing the scoring was the one part of a run
/// that cost nothing according to the run itself.
#[tokio::test]
async fn a_judge_call_is_priced_by_the_graders_own_pricing_block() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stop_reason": "tool_use",
            "model": "claude-x-20260101",
            "usage": {"input_tokens": 1_000_000, "output_tokens": 1_000_000},
            "content": [{
                "type": "tool_use", "name": "submit_verdict",
                "input": {"reasoning": "ok", "pass": true, "score": 1.0}
            }]
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");

    // `claude-x` is not in the built-in table, so without the override this
    // is unpriced — which is the second half of what this pins.
    let mut cfg = anthropic_grader(&server.uri());
    let unpriced = DefaultGrader::new(Some(cfg.clone()))
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap();
    assert!(unpriced.cost_usd.is_none(), "unknown model prices nothing");

    if let ProviderKind::Anthropic { pricing, .. } = &mut cfg.provider {
        *pricing = Some(Box::new(crate::config::PricingCfg {
            input_per_mtok: Some(3.0),
            output_per_mtok: Some(15.0),
            ..Default::default()
        }));
    }
    let graded = DefaultGrader::new(Some(cfg))
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap();
    assert_eq!(graded.cost_usd, Some(18.0));
    // The model the judge reported, not the one configured.
    assert_eq!(graded.model.as_deref(), Some("claude-x-20260101"));
}

/// A `similar` assertion with no embeddings provider fails closed rather than
/// grading against nothing.
///
/// The claim this replaces — "the verdict key moves with the embedding model" —
/// is now a property of the *request*, pinned by
/// `embeddings::tests::the_request_moves_with_the_model_and_never_carries_the_key_env`.
#[tokio::test]
async fn similar_without_an_embeddings_provider_fails_closed() {
    let assert = Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::Similar {
            value: crate::val::Val::Tpl(json!("hello")),
            threshold: None,
        },
    };
    let err = DefaultGrader::new(None)
        .grade(
            &assert,
            &Output::Text("hello".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GraderError::Unconfigured { kind: "similar" }));
}

#[tokio::test]
async fn truncated_verdict_fails_closed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stop_reason": "max_tokens",
            "content": []
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
    let outcome = grader
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await;
    let err = outcome.unwrap_err();
    // Asserting on the variant, not a substring: a truncated verdict must
    // fail closed *and* be identifiable as that specific problem, since the
    // fix (raise the grader's max_tokens) is unique to it.
    assert!(
        matches!(err, GraderError::TruncatedVerdict { .. }),
        "truncated verdict must fail closed as its own kind: {err}"
    );
}

/// A verdict the judge sampled badly is not a fact about the request, so the
/// grader asks again. Before this, one malformed object errored the case
/// permanently and — because an errored cell drives the infra exit code — took
/// the whole CI job down with it, reporting only "infrastructure error".
#[tokio::test]
async fn an_unusable_verdict_is_re_asked_and_recovers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            // Well-formed tool call, but `pass` is absent: exactly the shape
            // that took a 262-case suite red on two cells.
            "content": [{"type": "tool_use", "name": "submit_verdict",
                         "input": {"reasoning": "hmm", "score": 0.9}}]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "name": "submit_verdict",
                         "input": {"reasoning": "fine", "pass": true, "score": 0.9}}]
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
    let outcome = grader
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .expect("a re-ask should recover a sampled-bad verdict");
    assert!(
        matches!(
            outcome.verdict,
            crate::cache::GradedVerdict::Rubric { pass: true, .. }
        ),
        "{outcome:?}"
    );
}

/// Bounded, not infinite: a judge that is genuinely broken must still fail the
/// case rather than multiplying the grading bill until the job times out.
#[tokio::test]
async fn a_persistently_unusable_verdict_still_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "name": "submit_verdict",
                         "input": {"reasoning": "hmm", "score": 0.9}}]
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
    let err = grader
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GraderError::InvalidVerdict(_)), "{err}");
    // The message must say what was wrong and show the reply, because nothing
    // else records it: a payload that fails to parse is never cached, so a
    // re-run re-samples and the offending reply is gone.
    let msg = err.to_string();
    assert!(msg.contains("`pass`"), "{msg}");
    assert!(msg.contains("absent"), "{msg}");
    assert!(
        msg.contains("\"score\":0.9"),
        "the reply must be quoted: {msg}"
    );
}

/// The quoted reply must not carry the judge's `reasoning`, which restates the
/// graded output. This message reaches `case.error`, the shared run and the PR
/// comment, so quoting it verbatim would publish whatever was being graded.
#[tokio::test]
async fn the_quoted_verdict_redacts_the_judges_reasoning() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "name": "submit_verdict",
                         "input": {"reasoning": "the customer's SSN is 123-45-6789",
                                   "score": 0.9}}]
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
    let msg = grader
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        !msg.contains("123-45-6789"),
        "the graded content must not be republished: {msg}"
    );
    assert!(!msg.contains("customer"), "{msg}");
    // The shape still survives, which is what makes the message useful.
    assert!(msg.contains("redacted"), "{msg}");
    assert!(msg.contains("\"score\":0.9"), "{msg}");
    assert!(msg.contains("`pass`"), "{msg}");
}

/// A `pass` that is present but the wrong type used to report as *missing*,
/// sending a reader to look for a schema the judge had in fact received.
#[tokio::test]
async fn a_wrongly_typed_pass_says_so_rather_than_calling_it_missing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "name": "submit_verdict",
                         "input": {"reasoning": "hmm", "pass": "true", "score": 0.9}}]
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
    let msg = grader
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(msg.contains("`pass`"), "{msg}");
    assert!(msg.contains("a string"), "should name the type seen: {msg}");
    assert!(!msg.contains("absent"), "it is present, just wrong: {msg}");
}

/// The verdict is read from the judge's *own* tool. A stray tool block used to
/// be taken as the verdict and then reported as a missing `pass`.
#[tokio::test]
async fn a_tool_block_that_is_not_the_verdict_tool_is_not_read_as_one() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "name": "web_search",
                         "input": {"query": "is this polite"}}]
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
    let msg = grader
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(msg.contains("submit_verdict"), "{msg}");
    assert!(
        !msg.contains("`pass`"),
        "a stray tool must not masquerade as a schema problem: {msg}"
    );
}

#[tokio::test]
async fn no_grader_configured_fails_closed() {
    let grader = DefaultGrader::new(None);
    let outcome = grader
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await;
    let err = outcome.unwrap_err();
    assert!(
        matches!(err, GraderError::Unconfigured { kind: "llm-rubric" }),
        "an unconfigured grader is the suite author's problem, not a failure: {err}"
    );
    assert_eq!(err.class().as_str(), ErrorClass::GRADER_MISSING);
}

#[tokio::test]
async fn thinking_params_are_rejected() {
    let server = MockServer::start().await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let mut params = serde_json::Map::new();
    params.insert("thinking".into(), json!({"type": "enabled"}));
    let grader = Grader {
        provider: ProviderKind::Anthropic {
            model: "claude-x".into(),
            base_url: Some(server.uri()),
            api_key_env: Some("GRADER_TEST_KEY".into()),
            params: Some(params),
            pricing: None,
            request: None,
            cache_salt: None,
        },
        template: None,
        verdict_mode: None,
        timeout_ms: None,
        include_tool_calls: None,
    };
    let outcome = DefaultGrader::new(Some(grader))
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await;
    let err = outcome.unwrap_err();
    assert!(matches!(err, GraderError::Misconfigured(_)), "{err}");
    assert!(err.to_string().contains("thinking"), "{err}");
}

#[tokio::test]
async fn exec_assert_grades() {
    let assert = Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::Exec {
            command: vec![
                "sh".into(),
                "-c".into(),
                "cat >/dev/null; printf '{\"pass\":true,\"score\":1.0,\"reason\":\"ok\"}'".into(),
            ],
            config: None,
            cache_salt: None,
        },
    };
    let outcome = DefaultGrader::new(None)
        .grade(
            &assert,
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap()
        .verdict
        .to_outcome(None);
    assert!(outcome.passed);
}

// The three fields that were parsed, schema'd, documented, and never read.
// The three fields that were parsed, schema'd, documented, and never read.

fn params(pairs: &[(&str, serde_json::Value)]) -> ParamMap {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// `LlmRubric.params` is the more specific of the two, so it wins — a suite
/// sets it precisely to deviate from the shared grader default.
#[test]
fn assert_params_override_the_grader_provider_params() {
    let merged = merge_params(
        Some(&params(&[
            ("temperature", serde_json::json!(0)),
            ("max_tokens", serde_json::json!(1024)),
        ])),
        Some(&params(&[("max_tokens", serde_json::json!(8192))])),
    )
    .expect("merged");
    assert_eq!(merged.get("max_tokens"), Some(&serde_json::json!(8192)));
    // Keys the assertion did not mention are inherited, not dropped.
    assert_eq!(merged.get("temperature"), Some(&serde_json::json!(0)));
}

#[test]
fn merging_is_identity_when_only_one_side_is_set() {
    let only_provider = params(&[("a", serde_json::json!(1))]);
    assert_eq!(
        merge_params(Some(&only_provider), None).as_ref(),
        Some(&only_provider)
    );
    assert_eq!(
        merge_params(None, Some(&only_provider)).as_ref(),
        Some(&only_provider)
    );
    assert!(merge_params(None, None).is_none());
}

#[test]
fn a_grader_template_substitutes_both_placeholders() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("t.txt"),
        "Judge this.\n<r>{{rubric}}</r>\n<o>{{output}}</o>",
    )
    .unwrap();
    let rendered = render_grader_template(
        "file://t.txt",
        Some(dir.path()),
        "be concise",
        "a model answer",
        None,
    )
    .unwrap();
    assert!(rendered.contains("<r>be concise</r>"));
    assert!(rendered.contains("<o>a model answer</o>"));
}

/// The output is untrusted model text. Substituting literally rather than
/// rendering keeps a grading prompt from becoming a template-injection
/// surface — a model that emits `{{ ... }}` must not have it evaluated.
#[test]
fn a_grader_template_does_not_evaluate_the_output() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("t.txt"), "{{output}}").unwrap();
    let rendered =
        render_grader_template("file://t.txt", Some(dir.path()), "r", "{{ 7 * 7 }}", None).unwrap();
    assert_eq!(rendered, "{{ 7 * 7 }}", "must not evaluate to 49");
}

#[test]
fn a_non_file_template_is_a_config_error() {
    let err = render_grader_template("./relative.txt", None, "r", "o", None).unwrap_err();
    assert!(err.to_string().contains("file://"));
}

/// The path is relative to the *suite*, like every other `file://` a suite
/// can write. Reading it against the process cwd meant the documented form
/// worked only when you happened to run from the suite's own directory —
/// otherwise every llm-rubric assertion errored and the run exited 3.
#[test]
fn a_grader_template_resolves_against_the_suite_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("prompts")).unwrap();
    std::fs::write(dir.path().join("prompts/judge.md"), "J:{{rubric}}").unwrap();
    assert_eq!(
        render_grader_template("file://prompts/judge.md", Some(dir.path()), "r", "o", None)
            .unwrap(),
        "J:r"
    );
}

/// Same sandbox as every other `file://` reference. Without it a suite could
/// name any file on the machine and have its contents posted to the judge.
#[test]
fn a_traversing_grader_template_is_rejected() {
    let parent = tempfile::tempdir().unwrap();
    std::fs::write(parent.path().join("secret.txt"), "TOP SECRET").unwrap();
    let base = parent.path().join("suite");
    std::fs::create_dir(&base).unwrap();
    let err =
        render_grader_template("file://../secret.txt", Some(&base), "r", "o", None).unwrap_err();
    assert!(err.to_string().contains("refuses to read outside"), "{err}");
}

/// That file *is* the grading prompt, so editing it has to bust the cache —
/// and since 0.5.0 it does so by being *in the request* rather than through a
/// side-channel digest. This is the unit-level half of that claim: the same
/// rubric and output, two template files, two different judge bodies.
///
/// The deleted `template_digest` existed because the key hashed a fingerprint
/// that could not see the prompt. It can now, so the digest is gone and the
/// guarantee is structural. `tests/grader_request_cache.rs` pins the end of it:
/// editing the file makes the run call the judge again.
#[test]
fn editing_a_grader_template_moves_the_judge_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("judge.md");
    let body = || {
        let user = render_grader_template(
            "file://judge.md",
            Some(dir.path()),
            "be good",
            "an answer",
            None,
        )
        .unwrap();
        Judge::Anthropic
            .request("judge", None, None, &user, &judge_default())
            .body
    };

    std::fs::write(&path, "Be lenient. {{rubric}} {{output}}").unwrap();
    let before = body();
    std::fs::write(&path, "Be strict. {{rubric}} {{output}}").unwrap();
    assert_ne!(before, body(), "the template's bytes are in the body");
}

/// The judge request carries everything the deleted `grading_fingerprint`
/// enumerated by hand, so each of those inputs still separates two gradings —
/// and the credential still does not appear, because it is a header.
///
/// `base_url` is the one member that left: it addresses the request rather than
/// asking it. See `a_judge_behind_a_gateway_keys_the_same_as_one_going_direct`.
#[test]
fn the_judge_request_separates_what_the_fingerprint_used_to() {
    let base = |model: &str, url: Option<&str>, params: Option<&ParamMap>, user: &str| {
        Judge::Anthropic.request(model, url, params, user, &judge_default())
    };
    let reference = base("judge", None, None, "RUBRIC:\nbe good");
    for other in [
        base("judge-2", None, None, "RUBRIC:\nbe good"),
        base(
            "judge",
            None,
            Some(&params(&[("temperature", json!(0.5))])),
            "RUBRIC:\nbe good",
        ),
        // The rendered rubric and the graded output both live in `user`.
        base("judge", None, None, "RUBRIC:\nbe concise"),
    ] {
        assert_ne!(reference, other, "these gradings must not share an entry");
    }
    // The system prompt is in the body, so an edit to it would move every key.
    assert_eq!(reference.body["system"], json!(SYSTEM_PROMPT));
    // …and no credential is anywhere in it.
    assert!(
        !serde_json::to_string(&reference.body)
            .unwrap()
            .contains("key"),
        "{:?}",
        reference.body
    );
}

/// The deliberate exception to the rule above, and the reason `base_url` is not
/// in `path`: two judges differing only in where the request is *addressed* are
/// asking the same question, so they share an entry. A team behind a gateway and
/// a teammate going direct must not pay for the same grading twice. When two
/// endpoints genuinely answer differently, `cache_salt` separates them.
#[test]
fn a_judge_behind_a_gateway_keys_the_same_as_one_going_direct() {
    let direct =
        Judge::Anthropic.request("judge", None, None, "RUBRIC:\nbe good", &judge_default());
    let gateway = Judge::Anthropic.request(
        "judge",
        Some("https://llm-gw.corp.internal/anthropic"),
        None,
        "RUBRIC:\nbe good",
        &judge_default(),
    );

    assert_ne!(
        direct.url, gateway.url,
        "they are posted to different hosts"
    );
    assert_eq!(
        direct.path, gateway.path,
        "but keyed identically, so the cache is shared"
    );
    assert_eq!(direct.body, gateway.body);
}

/// The opt-in's whole point: with the flag unset, a cell that made tool calls
/// produces the same prompt bytes it produced before the flag existed. The
/// judge's request body *is* the cache key, so anything else would re-grade
/// every warm entry in every store the first time a suite reported a call.
#[test]
fn the_grading_prompt_is_unchanged_when_tool_calls_are_not_opted_into() {
    let expected = format!(
        "RUBRIC:\n{}\n\nASSISTANT OUTPUT:\n{}",
        "be good", "an answer"
    );
    let mut grader = anthropic_grader("http://judge.test");
    for flag in [None, Some(false)] {
        grader.include_tool_calls = flag;
        let user =
            grading_user_message(&grader, None, "be good", "an answer", &one_tool_call()).unwrap();
        assert_eq!(user, expected, "flag {flag:?} must not touch the prompt");
    }
}

/// The id is a random per-response vendor token (`toolu_…`). Including it
/// would put a fresh string in every judge request body — and therefore a
/// fresh cache key — for a decision the model made identically twice.
#[test]
fn opting_in_shows_the_judge_the_calls_but_never_the_vendor_call_id() {
    let mut grader = anthropic_grader("http://judge.test");
    grader.include_tool_calls = Some(true);
    let user =
        grading_user_message(&grader, None, "be good", "an answer", &one_tool_call()).unwrap();
    assert!(
        user.starts_with("RUBRIC:\nbe good\n\nASSISTANT OUTPUT:\nan answer\n\nTOOL CALLS (the tool calls the assistant made, in order, as JSON):\n"),
        "{user}"
    );
    assert!(user.contains("get_weather"), "{user}");
    assert!(user.contains("Reykjavik"), "{user}");
    assert!(
        !user.contains("toolu"),
        "the vendor id must not be sent: {user}"
    );
    assert!(
        !user.contains("\"id\""),
        "the vendor id must not be sent: {user}"
    );
}

/// Absence is a gradeable fact: a rubric that says "must call `search`" can
/// only fail the cell if the section is there to be empty.
#[test]
fn the_tool_calls_section_is_present_even_when_the_model_called_nothing() {
    let mut grader = anthropic_grader("http://judge.test");
    grader.include_tool_calls = Some(true);
    let user = grading_user_message(&grader, None, "be good", "an answer", &[]).unwrap();
    assert!(user.ends_with("as JSON):\n[]"), "{user}");
}

#[test]
fn a_grader_template_substitutes_the_tool_calls_placeholder_when_opted_in() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("t.txt"),
        "<r>{{rubric}}</r>\n<o>{{output}}</o>\n<t>{{tool_calls}}</t>",
    )
    .unwrap();
    let rendered = render_grader_template(
        "file://t.txt",
        Some(dir.path()),
        "be concise",
        "a model answer",
        Some("[]"),
    )
    .unwrap();
    assert!(rendered.contains("<r>be concise</r>"), "{rendered}");
    assert!(rendered.contains("<o>a model answer</o>"), "{rendered}");
    assert!(rendered.contains("<t>[]</t>"), "{rendered}");
}

/// Both settings come out of the same authored `grader:` block, so either
/// direction of disagreement is a contradiction the author can fix — and
/// silently dropping the placeholder (or silently omitting the calls) would
/// hand the judge a prompt nobody asked for.
#[test]
fn a_tool_calls_placeholder_without_the_opt_in_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("t.txt"), "{{rubric}} {{tool_calls}}").unwrap();
    let err = render_grader_template("file://t.txt", Some(dir.path()), "r", "o", None).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("include_tool_calls"), "{msg}");
    assert!(msg.contains("{{tool_calls}}"), "{msg}");
}

#[test]
fn opting_in_without_a_tool_calls_placeholder_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("t.txt"), "{{rubric}} {{output}}").unwrap();
    let err =
        render_grader_template("file://t.txt", Some(dir.path()), "r", "o", Some("[]")).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("include_tool_calls"), "{msg}");
    assert!(msg.contains("{{tool_calls}}"), "{msg}");
}

/// One left-to-right pass, and what a placeholder expands to is never scanned
/// again. Chained `str::replace` calls rescan: a rubric containing the literal
/// `{{output}}` had the model's answer spliced into it, which is the rubric
/// author losing control of the rubric.
#[test]
fn a_substituted_value_is_never_rescanned_for_placeholders() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("t.txt"), "{{rubric}}|{{output}}").unwrap();
    let rendered = render_grader_template(
        "file://t.txt",
        Some(dir.path()),
        "grade against {{output}}",
        "ANSWER",
        None,
    )
    .unwrap();
    assert_eq!(rendered, "grade against {{output}}|ANSWER");
}

/// The same rule pointed at the untrusted half: model text that happens to
/// contain `{{tool_calls}}` is prose, not a placeholder.
#[test]
fn an_output_containing_the_tool_calls_placeholder_is_not_expanded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("t.txt"), "{{output}}{{tool_calls}}").unwrap();
    let rendered = render_grader_template(
        "file://t.txt",
        Some(dir.path()),
        "r",
        "{{tool_calls}}",
        Some("[1]"),
    )
    .unwrap();
    assert_eq!(rendered, "{{tool_calls}}[1]");
}

/// The flag and the calls are both in the judge's body, so both separate two
/// gradings — while the vendor call id, which is not in the body, does not.
#[test]
fn the_judge_request_separates_gradings_that_saw_different_tool_calls() {
    let body = |grader: &Grader, calls: &[crate::result::ToolCall]| {
        let user = grading_user_message(grader, None, "be good", "an answer", calls).unwrap();
        Judge::Anthropic
            .request("judge", None, None, &user, &judge_default())
            .body
    };
    let off = anthropic_grader("http://judge.test");
    let mut on = off.clone();
    on.include_tool_calls = Some(true);

    let calls = one_tool_call();
    let elsewhere = vec![crate::result::ToolCall {
        id: Some("toolu_01ABCDEF".into()),
        name: "get_weather".into(),
        arguments: json!({"city": "Vancouver"}),
    }];
    assert_ne!(
        body(&off, &calls),
        body(&on, &calls),
        "the flag is in the body"
    );
    assert_ne!(
        body(&on, &calls),
        body(&on, &elsewhere),
        "the calls are too"
    );
    assert_ne!(body(&on, &calls), body(&on, &[]), "so is their absence");

    // The same decision reported with a fresh vendor id is the same grading,
    // and must stay one cache entry across live provider runs.
    let same_decision = vec![crate::result::ToolCall {
        id: Some("toolu_99ZZZZZZ".into()),
        ..calls[0].clone()
    }];
    assert_eq!(body(&on, &calls), body(&on, &same_decision));
    assert_eq!(body(&on, &calls)["system"], json!(SYSTEM_PROMPT));
}

/// End to end against a judge: what the section promises has to survive the
/// whole path, not just the formatter.
#[tokio::test]
async fn the_judge_sees_the_tool_calls_only_when_the_suite_opts_in() {
    for (include, expect_section) in [(Some(true), true), (None, false)] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use", "name": "submit_verdict",
                    "input": {"reasoning": "it called it", "pass": true, "score": 1.0}
                }]
            })))
            .mount(&server)
            .await;
        std::env::set_var("GRADER_TEST_KEY", "sk-test");
        let mut cfg = anthropic_grader(&server.uri());
        cfg.include_tool_calls = include;
        let calls = one_tool_call();
        DefaultGrader::new(Some(cfg))
            .grade(
                &rubric_assert(),
                &Output::Text("".into()),
                &grade_ctx_with_calls(&json!({}), &TemplateEngine::new(), &calls),
            )
            .await
            .unwrap();

        let sent = server.received_requests().await.unwrap();
        let body = String::from_utf8(sent[0].body.clone()).unwrap();
        assert_eq!(body.contains("TOOL CALLS"), expect_section, "{body}");
        assert_eq!(body.contains("get_weather"), expect_section, "{body}");
        assert_eq!(body.contains("Reykjavik"), expect_section, "{body}");
    }
}

/// The child gets the calls with no flag to set: the exec protocol adds fields
/// additively, and a child that does not read them is unaffected.
#[tokio::test]
async fn an_exec_assert_is_told_the_cells_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("stdin.json");
    let calls = one_tool_call();
    let outcome = DefaultGrader::new(None)
        .grade(
            &exec_assert_capturing_stdin(&seen),
            &Output::Text("x".into()),
            &grade_ctx_with_calls(&json!({}), &TemplateEngine::new(), &calls),
        )
        .await
        .unwrap()
        .verdict
        .to_outcome(None);
    assert!(outcome.passed);

    let request: Json = serde_json::from_str(&std::fs::read_to_string(&seen).unwrap()).unwrap();
    assert_eq!(request["tool_calls"][0]["name"], json!("get_weather"));
    assert_eq!(
        request["tool_calls"][0]["arguments"]["city"],
        json!("Reykjavik")
    );
}

/// The other half of "additive": a tool-less cell's stdin — and therefore its
/// cache key — is byte-identical to what a pre-0.6 domarinn wrote.
#[tokio::test]
async fn an_exec_assert_request_omits_tool_calls_when_there_were_none() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("stdin.json");
    DefaultGrader::new(None)
        .grade(
            &exec_assert_capturing_stdin(&seen),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .unwrap();

    let request: Json = serde_json::from_str(&std::fs::read_to_string(&seen).unwrap()).unwrap();
    assert!(
        request.get("tool_calls").is_none(),
        "an empty list must not appear on the wire: {request}"
    );
}

/// A shell judge that keeps what it was sent, so a test can read the request
/// the child actually received rather than the struct domarinn built.
fn exec_assert_capturing_stdin(seen: &std::path::Path) -> Assert {
    Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::Exec {
            command: vec![
                "sh".into(),
                "-c".into(),
                format!(
                    "cat >'{}'; printf '{{\"pass\":true,\"score\":1.0,\"reason\":\"ok\"}}'",
                    seen.display()
                ),
            ],
            config: None,
            cache_salt: None,
        },
    }
}
