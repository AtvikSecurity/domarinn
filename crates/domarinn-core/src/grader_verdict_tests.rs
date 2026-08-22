//! The verdict-shape tests for [`super::super`] (the default grader): what a
//! judge's reply may look like, how an unusable one is re-asked, and what the
//! resulting error message may and may not carry. Split from
//! `grader_tests.rs` — whose helpers these tests share via `use super::*` —
//! to keep both files under the repo's 1000-line source cap.

use super::*;

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

/// The graded text does not arrive under a well-known key. A judge that broke
/// schema restates it under whatever name it invents — so redaction is
/// structural (every string, wherever it sits), not a denylist of one key.
#[tokio::test]
async fn the_quoted_verdict_redacts_strings_under_any_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "name": "submit_verdict",
                         "input": {"explanation": "the customer's SSN is 123-45-6789",
                                   "verdict": {"quote": "SSN 123-45-6789 appears"},
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
        "an alternate or nested key must not leak the graded content: {msg}"
    );
    // The shape — keys, types, the numeric score — still reads.
    assert!(msg.contains("explanation"), "{msg}");
    assert!(msg.contains("redacted"), "{msg}");
    assert!(msg.contains("\"score\":0.9"), "{msg}");
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

/// Several tool blocks, none of them the verdict: ambiguous, so the parser
/// refuses with the tool names rather than guessing — the old behavior read
/// the *first* block of any name and then reported a missing `pass`, sending a
/// reader to hunt for a schema bug that was not there.
#[tokio::test]
async fn competing_stray_tool_blocks_are_refused_by_name() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [
                {"type": "tool_use", "name": "web_search",
                 "input": {"query": "is this polite"}},
                {"type": "tool_use", "name": "code_exec",
                 "input": {"code": "1+1"}}
            ]
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
        msg.contains("web_search"),
        "the stray names are the diagnosis: {msg}"
    );
    assert!(
        !msg.contains("`pass`"),
        "a stray tool must not masquerade as a schema problem: {msg}"
    );
}

/// A *sole* tool block under another name is still the verdict.
///
/// A gateway that requires a differently-named tool is wired in via the
/// grader's `request.body` override, and every reply — including payloads
/// cached before an upgrade, which `--cache-only` cannot re-fetch — carries
/// that name. Forced tool choice makes the sole block unambiguous; strict
/// name-matching here turned those suites into hard errors on every case.
#[tokio::test]
async fn a_sole_renamed_tool_block_is_still_the_verdict() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "name": "record_verdict",
                         "input": {"pass": true, "score": 1.0}}]
        })))
        .mount(&server)
        .await;
    std::env::set_var("GRADER_TEST_KEY", "sk-test");
    let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
    let graded = grader
        .grade(
            &rubric_assert(),
            &Output::Text("x".into()),
            &grade_ctx(&json!({}), &TemplateEngine::new()),
        )
        .await
        .expect("a sole forced tool call is the verdict, whatever its name");
    assert!(matches!(
        graded.verdict,
        crate::grader::GradedVerdict::Rubric { pass: true, .. }
    ));
}
