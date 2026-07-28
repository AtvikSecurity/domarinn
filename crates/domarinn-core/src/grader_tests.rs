//! Unit tests for [`super`] (the default grader). Split out of `grader.rs`
//! via `#[path]` to keep that file under the repo's 1000-line source cap;
//! this is still the grader's private child module (`use super::*`).

use super::*;
use crate::config::Grader;
use crate::config::ParamMap;
use crate::error_class::ErrorClass;
use crate::errors::Classify;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Identity for a cell under test. The values are arbitrary; what matters
/// is that they are no longer empty strings on the wire.
fn grade_ctx<'a>(vars: &'a Json, engine: &'a TemplateEngine) -> GradeCtx<'a> {
    GradeCtx {
        vars,
        engine,
        working_dir: None,
        provider_id: "p",
        test_id: "t",
        test_tags: &[],
    }
}

fn anthropic_grader(uri: &str) -> Grader {
    Grader {
        provider: ProviderKind::Anthropic {
            model: "claude-x".into(),
            base_url: Some(uri.to_string()),
            api_key_env: Some("GRADER_TEST_KEY".into()),
            params: None,
            pricing: None,
        },
        template: None,
        verdict_mode: None,
        timeout_ms: None,
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

/// A cosine value belongs to the model that produced the vectors, so the
/// verdict key must move when the embedder does. It did not: the key was the
/// constant `{"assert": "similar"}`, so switching embedding models replayed
/// the previous one's answers.
#[test]
fn a_similar_verdict_key_moves_with_the_embedding_model() {
    let assert = Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::Similar {
            value: crate::val::Val::Tpl(json!("hello")),
            threshold: None,
        },
    };
    let with = |model: &str| {
        DefaultGrader::new(None)
            .with_embeddings(crate::embeddings::EmbeddingsProvider::new(
                "e", model, None, None, None, None,
            ))
            .grading_fingerprint(&assert, None)
    };
    assert_ne!(
        with("text-embedding-3-small"),
        with("text-embedding-3-large")
    );
    // And with no embeddings provider there is nothing to key on, so the
    // assertion opts out of caching entirely rather than caching an error.
    assert!(DefaultGrader::new(None)
        .grading_fingerprint(&assert, None)
        .is_none());
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
        },
        template: None,
        verdict_mode: None,
        timeout_ms: None,
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
        render_grader_template("file://t.txt", Some(dir.path()), "r", "{{ 7 * 7 }}").unwrap();
    assert_eq!(rendered, "{{ 7 * 7 }}", "must not evaluate to 49");
}

#[test]
fn a_non_file_template_is_a_config_error() {
    let err = render_grader_template("./relative.txt", None, "r", "o").unwrap_err();
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
        render_grader_template("file://prompts/judge.md", Some(dir.path()), "r", "o").unwrap(),
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
    let err = render_grader_template("file://../secret.txt", Some(&base), "r", "o").unwrap_err();
    assert!(err.to_string().contains("refuses to read outside"), "{err}");
}

/// That file *is* the grading prompt on this branch, so editing it has to
/// bust the verdict cache. Keying on the path alone replayed scores produced
/// by the previous judging prompt, with no cache miss and no warning.
#[test]
fn editing_a_grader_template_moves_the_verdict_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("judge.md");
    let assert = Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::LlmRubric {
            value: "good".into(),
            grader: None,
            threshold: None,
            params: None,
        },
    };
    let grader = |_: ()| Grader {
        provider: ProviderKind::Anthropic {
            model: "judge".into(),
            base_url: None,
            api_key_env: None,
            params: None,
            pricing: None,
        },
        template: Some("file://judge.md".into()),
        verdict_mode: None,
        timeout_ms: None,
    };
    let fingerprint = || {
        DefaultGrader::new(Some(grader(())))
            .grading_fingerprint(&assert, Some(dir.path()))
            .unwrap()
    };

    std::fs::write(&path, "Be lenient. {{rubric}} {{output}}").unwrap();
    let before = fingerprint();
    std::fs::write(&path, "Be strict. {{rubric}} {{output}}").unwrap();
    assert_ne!(before, fingerprint());
}
