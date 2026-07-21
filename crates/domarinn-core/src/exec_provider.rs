//! The `exec` provider: a system under test invoked as an external command.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::exec::run_exec_json;
use crate::exec_protocol::{Envelope, Kind, ProviderReq, TestRef};
use crate::provider::{CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse};
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

    fn cacheable(&self) -> bool {
        self.cache_salt.is_some()
    }

    async fn call(
        &self,
        req: &ProviderRequest,
        ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError> {
        let prompt_json = req.prompt.as_ref().map(rendered_prompt_json);
        let request = ProviderReq {
            envelope: Envelope::new(Kind::Provider),
            prompt: prompt_json,
            vars: Json::Object(req.vars.clone().into_iter().collect()),
            params: Json::Object(req.params.clone()),
            test: TestRef {
                id: req.test.id.clone(),
                tags: req.test.tags.clone(),
            },
        };
        let request = serde_json::to_value(&request)
            .map_err(|e| ProviderError::Fatal(anyhow::anyhow!("serializing exec request: {e}")))?;

        let cwd = ctx.working_dir.as_deref();
        let response = run_exec_json(&self.command, &self.env, cwd, self.timeout, &request)
            .await
            .map_err(|e| {
                if e.is_retriable() {
                    ProviderError::Retriable {
                        source: anyhow::Error::new(e),
                        retry_after: None,
                    }
                } else {
                    ProviderError::Fatal(anyhow::Error::new(e))
                }
            })?;

        parse_response(response)
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
    let resp: crate::exec_protocol::ProviderResp = serde_json::from_value(value)
        .map_err(|e| ProviderError::Fatal(anyhow::anyhow!("bad provider response: {e}")))?;

    if let Some(err) = resp.error {
        return Err(if err.retriable {
            ProviderError::Retriable {
                source: anyhow::anyhow!(err.message),
                retry_after: None,
            }
        } else {
            ProviderError::Fatal(anyhow::anyhow!(err.message))
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
        raw: resp.metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TestMeta;

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
