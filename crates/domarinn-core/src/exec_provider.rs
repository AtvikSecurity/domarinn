//! The `exec` provider: a system under test invoked as an external command.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::error_class::ErrorClass;
use crate::exec::run_exec_json;
use crate::exec_protocol::{Envelope, Kind, ProviderReq, TestRef};
use crate::provider::{
    exec_request_preview, CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse,
};
use crate::types::{Output, RenderedPrompt, TokenUsage};

const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// A provider that shells out to a command speaking the exec JSON protocol.
pub struct ExecProvider {
    id: String,
    command: Vec<String>,
    env: BTreeMap<String, String>,
    timeout: Duration,
    cache_salt: Option<String>,
}

impl ExecProvider {
    pub fn new(
        id: impl Into<String>,
        command: Vec<String>,
        env: BTreeMap<String, String>,
        timeout_ms: Option<u64>,
        cache_salt: Option<String>,
    ) -> Self {
        ExecProvider {
            id: id.into(),
            command,
            env,
            timeout: Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
            cache_salt,
        }
    }
}

#[async_trait]
impl Provider for ExecProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn fingerprint(&self) -> Json {
        serde_json::json!({
            "type": "exec",
            "command": self.command,
            "cache_salt": self.cache_salt,
        })
    }

    /// Keyed on the **provider** salt only, deliberately.
    ///
    /// Do *not* relax this to also accept a case's `cache_salt`. That is a
    /// tempting "fix" when a suite sets case salts and sees no caching, but the
    /// two salts answer different questions: this one is *"is this the same
    /// build?"*, a case salt is only *"is this the same content?"*. A case salt
    /// digests prompt content, which does not move when the binary behind
    /// `command` is rebuilt — so accepting it here would serve stale output from
    /// every entry after a rebuild, silently, and worst of all in CI. No caching
    /// is the correct answer for a salted case with no provider salt; the runner
    /// warns about that combination instead of papering over it.
    fn cacheable(&self) -> bool {
        self.cache_salt.is_some()
    }

    async fn call(
        &self,
        req: &ProviderRequest,
        ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError> {
        let request = serde_json::to_value(protocol_request(req)).map_err(|e| {
            ProviderError::fatal(
                ErrorClass::EXEC_FAILED,
                anyhow::anyhow!("serializing exec request: {e}"),
            )
        })?;

        let cwd = ctx.working_dir.as_deref();
        let response = run_exec_json(&self.command, &self.env, cwd, self.timeout, &request)
            .await
            .map_err(|e| {
                if e.is_retriable() {
                    ProviderError::retriable(ErrorClass::EXEC_FAILED, anyhow::Error::new(e), None)
                } else {
                    ProviderError::fatal(ErrorClass::EXEC_FAILED, anyhow::Error::new(e))
                }
            })?;

        parse_response(response)
    }

    /// The exec protocol document this provider writes to the child's stdin.
    ///
    /// Byte-identical to the real call: [`Envelope`] carries only a protocol
    /// version and a kind, so nothing here is generated per invocation. The
    /// provider's own `env` map is deliberately excluded — it is a credential
    /// channel, and this value is persisted into a shareable run document.
    fn request_preview(&self, req: &ProviderRequest) -> Option<Json> {
        let stdin = serde_json::to_value(protocol_request(req)).ok()?;
        let (command, args) = self.command.split_first()?;
        Some(exec_request_preview(command, args, stdin))
    }
}

/// Build the exec-protocol request for one provider call.
///
/// `req.case_salt` is deliberately absent: it is a cache-keying concern, and
/// forwarding it would leak the suite's digest into the child's input (and
/// change the SUT's payload). Do not "complete" this mapping by adding it.
fn protocol_request(req: &ProviderRequest) -> ProviderReq {
    ProviderReq {
        envelope: Envelope::new(Kind::Provider),
        prompt: req.prompt.as_ref().map(rendered_prompt_json),
        vars: Json::Object(req.vars.clone().into_iter().collect()),
        params: Json::Object(req.params.clone()),
        test: TestRef {
            id: req.test.id.clone(),
            tags: req.test.tags.clone(),
        },
    }
}

/// Serialize a rendered prompt into the exec protocol's `prompt` shape.
fn rendered_prompt_json(prompt: &RenderedPrompt) -> Json {
    match prompt {
        RenderedPrompt::Text(t) => serde_json::json!({ "text": t }),
        RenderedPrompt::Messages(msgs) => serde_json::json!({ "messages": msgs }),
    }
}

/// Parse a provider protocol response into a [`ProviderResponse`], surfacing a
/// clean provider-level error if the child reported one.
fn parse_response(value: Json) -> Result<ProviderResponse, ProviderError> {
    let resp: crate::exec_protocol::ProviderResp = serde_json::from_value(value).map_err(|e| {
        ProviderError::fatal(
            ErrorClass::EXEC_FAILED,
            anyhow::anyhow!("bad provider response: {e}"),
        )
    })?;

    if let Some(err) = resp.error {
        return Err(if err.retriable {
            // The child said it was retriable; it did not say why, and a
            // future child may name its own class here.
            ProviderError::retriable(ErrorClass::EXEC_FAILED, anyhow::anyhow!(err.message), None)
        } else {
            ProviderError::fatal(ErrorClass::EXEC_FAILED, anyhow::anyhow!(err.message))
        });
    }

    let output = match resp.output {
        Json::String(s) => Output::Text(s),
        other => Output::Json(other),
    };
    let usage = resp.usage.map(|u| TokenUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cache_read_tokens: None,
    });

    Ok(ProviderResponse {
        output,
        usage,
        cost_usd: resp.cost_usd,
        stop_reason: None,
        reasoning: None,
        empty_reason: None,
        raw: resp.metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TestMeta;
    use serde_json::json;

    /// See the matching test in `anthropic.rs`: the fingerprint feeds every
    /// cache key, so an unconditional change invalidates every cached entry.
    #[test]
    fn fingerprint_is_stable_for_default_config() {
        let p = ExecProvider::new("p", vec!["./sut".into()], BTreeMap::new(), None, None);
        assert_eq!(
            crate::cache::canonical_json(&p.fingerprint()),
            r#"{"cache_salt":null,"command":["./sut"],"type":"exec"}"#
        );
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

    #[test]
    fn request_preview_is_absent_for_an_empty_command() {
        let p = ExecProvider::new("e", Vec::new(), BTreeMap::new(), None, None);
        assert!(p.request_preview(&request("hi")).is_none());
    }

    fn request(user_input: &str) -> ProviderRequest {
        let mut vars = BTreeMap::new();
        vars.insert("user_input".to_string(), Json::String(user_input.into()));
        ProviderRequest {
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
    async fn cacheable_requires_cache_salt() {
        let with_salt = ExecProvider::new(
            "p",
            vec!["true".into()],
            BTreeMap::new(),
            None,
            Some("v1".into()),
        );
        let without = ExecProvider::new("p", vec!["true".into()], BTreeMap::new(), None, None);
        assert!(with_salt.cacheable());
        assert!(!without.cacheable());
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
        );
        let err = provider
            .call(&request("x"), &CallCtx::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Retriable { .. }));
    }
}
