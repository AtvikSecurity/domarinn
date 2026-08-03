//! The `exec` provider: a system under test invoked as an external command.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::empty::EmptyReason;
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
    /// Digest of `env`, so two providers differing only in their environment do
    /// not share a key. A digest rather than the map itself because a
    /// fingerprint is persisted into the cache entry and `env` is a credential
    /// channel — see [`Self::request_preview`], which excludes it for the same
    /// reason.
    env_digest: Option<String>,
    /// Evidence of which build answered, carried on the entry and **never** in
    /// the key — see [`crate::exec::program_digest`]. Resolved once at
    /// construction so a cache lookup costs no filesystem access.
    program_digest: Option<String>,
    /// Fingerprints this provider used to publish, so entries written by an
    /// older domarinn can be adopted rather than re-paid for. Deletable — see
    /// [`crate::cache_migrate`].
    legacy_fingerprints: Vec<Json>,
}

impl ExecProvider {
    /// `base_dir` is the directory children are spawned in — the suite's, in a
    /// real run. Relative arguments resolve against it when the program's
    /// digest is taken, so passing `None` (tests, embedders with no suite on
    /// disk) means only absolute and `PATH` programs contribute to it.
    ///
    /// Note that `base_dir` reaches neither the fingerprint nor the cache key.
    /// Where a checkout happens to live is a property of the machine, and the
    /// whole point of the current key shape is that no such property is in it.
    pub fn new(
        id: impl Into<String>,
        command: Vec<String>,
        env: BTreeMap<String, String>,
        timeout_ms: Option<u64>,
        cache_salt: Option<String>,
        base_dir: Option<&std::path::Path>,
    ) -> Self {
        let env_digest = env_digest(&env);
        ExecProvider {
            id: id.into(),
            program_digest: crate::exec::program_digest(&command, base_dir),
            legacy_fingerprints: crate::cache_migrate::legacy_exec_fingerprints(
                &command,
                env_digest.as_deref(),
                cache_salt.as_deref(),
                base_dir,
            ),
            env_digest,
            command,
            env,
            timeout: Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
            cache_salt,
        }
    }
}

/// A digest of a child's environment, or `None` when it sets nothing.
///
/// `None` rather than the digest of an empty map so the overwhelmingly common
/// no-`env` provider keeps a fingerprint a human can read at a glance.
fn env_digest(env: &BTreeMap<String, String>) -> Option<String> {
    if env.is_empty() {
        return None;
    }
    let canonical = crate::cache::canonical_json(&serde_json::json!(env));
    Some(format!(
        "blake3:{}",
        blake3::hash(canonical.as_bytes()).to_hex()
    ))
}

#[async_trait]
impl Provider for ExecProvider {
    fn id(&self) -> &str {
        &self.id
    }

    /// What *selects* this provider, and nothing else.
    ///
    /// `command` and `env` play exactly the part `model` and `base_url` play in
    /// the `anthropic` fingerprint: they name the thing that will answer. The
    /// question itself — the rendered prompt, the vars, the tools — reaches the
    /// cache key through [`Self::canonical_request`]; this value is frozen
    /// history plus run-diff provenance, per [`Provider::fingerprint`].
    ///
    /// There is deliberately no member describing the program's *bytes*. One
    /// used to exist, and it made an `exec` fingerprint a property of the local
    /// filesystem: a fresh checkout, a different working directory, or a CI
    /// runner that compiled its own provider could never match anything another
    /// machine had written. See [`crate::exec::program_digest`] for the full
    /// argument and for where that evidence lives now.
    fn fingerprint(&self) -> Json {
        serde_json::json!({
            "type": "exec",
            "command": self.command,
            // Two providers wrapping one script and differing only in `env` are
            // a normal A/B shape (`MODEL_ENDPOINT: http://a` vs `…/b`). Without
            // this they share a key and the second column silently replays the
            // first's answers, fabricating the comparison the run exists for.
            "env": self.env_digest,
            "cache_salt": self.cache_salt,
        })
    }

    /// Always. An `exec` provider is cached like every other kind.
    ///
    /// This was conditional twice. First on a hand-written `cache_salt`, then on
    /// domarinn finding a program on disk to hash. Both were standing in for the
    /// same worry — that argv does not move when the binary behind it is rebuilt
    /// — and both answered it by spending the entire cache to detect an event
    /// the suite already knows about. `cache_salt` remains the way to say "this
    /// is a different build"; a rebuild that does not say so is *reported* on the
    /// hit rather than pre-emptively paid for. See [`crate::exec::program_digest`].
    fn cacheable(&self) -> bool {
        true
    }

    fn program_digest(&self) -> Option<&str> {
        self.program_digest.as_deref()
    }

    /// The ≤0.4.0 shape — this provider's own current fingerprint — followed by
    /// the four older generations. See [`crate::cache_migrate`].
    fn legacy_fingerprints(&self) -> Vec<Json> {
        let mut shapes = Vec::with_capacity(1 + self.legacy_fingerprints.len());
        shapes.push(self.fingerprint());
        shapes.extend(self.legacy_fingerprints.iter().cloned());
        shapes
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

    /// The document written to stdin, minus the one member that identifies the
    /// *case* rather than the question, plus the digest of the environment the
    /// child runs in.
    ///
    /// `test` is stripped because a test id and its tags are correlation
    /// metadata, like a request id: two cases with identical vars are asking the
    /// same thing and must keep sharing an entry. The real call still sends it —
    /// this is a keying view of the request, not a second version of it.
    ///
    /// The `domarinn` envelope is **deliberately kept**, and that makes
    /// `PROTOCOL_VERSION` a cache-key member. It is sent, and a child is
    /// entitled to answer a v2 request differently from a v1 one, so two
    /// protocol versions are not the same question. The consequence is a flag
    /// day: bumping `PROTOCOL_VERSION` re-keys every `exec` entry in every
    /// store, and it MUST ship with the protocol-1 shape frozen as a new legacy
    /// generation in [`crate::cache_migrate`] — otherwise every warm exec cache
    /// in the world goes quiet at once, with no adoption path back.
    /// `bumping_the_exec_protocol_version_re_keys_every_exec_entry` in
    /// `cache_key.rs` is the tripwire that says so at the moment of the bump.
    ///
    /// `env_digest` is present only when the provider declares `env:`, and is a
    /// digest rather than the map because this value is persisted into every
    /// cache entry and `env` is where an exec provider's credentials live.
    fn canonical_request(&self, req: &ProviderRequest) -> Option<Json> {
        let mut stdin = serde_json::to_value(protocol_request(req)).ok()?;
        stdin.as_object_mut()?.remove("test");
        let (command, args) = self.command.split_first()?;
        let mut canonical = exec_request_preview(command, args, stdin);
        if let Some(digest) = &self.env_digest {
            canonical
                .as_object_mut()
                .expect("json! object literal")
                .insert("env_digest".to_string(), Json::String(digest.clone()));
        }
        Some(canonical)
    }

    fn cache_salt(&self) -> Option<&str> {
        self.cache_salt.as_deref()
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
        tools: req
            .tools
            .iter()
            .map(|t| domarinn_protocol::ToolDef {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone().unwrap_or(Json::Null),
            })
            .collect(),
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
        // `metadata` as the fallback fixes a real loss: this arm used to return
        // before `metadata` was read at all, so a child that sent structured
        // diagnostics alongside an error had them silently discarded — which is
        // exactly why children ended up formatting JSON into `message`.
        let details = err.details.or(resp.metadata);
        // The child's own class when it named one, so a rejected credential or
        // a rate limit stays distinguishable instead of collapsing into
        // "exec_failed" like every other exec failure. Unknown values are kept
        // verbatim: `ErrorClass` is open by construction, and rejecting a
        // future vocabulary here would turn a diagnosis into a parse failure.
        let class = err.class.as_deref().unwrap_or(ErrorClass::EXEC_FAILED);
        return Err(if err.retriable {
            let retry_after = err.retry_after_ms.map(Duration::from_millis);
            ProviderError::retriable(class, anyhow::anyhow!(err.message), retry_after)
                .with_details(details)
        } else {
            ProviderError::fatal(class, anyhow::anyhow!(err.message)).with_details(details)
        });
    }

    let output = match resp.output {
        Json::String(s) => Output::Text(s),
        other => Output::Json(other),
    };
    let usage = resp.usage.map(|u| TokenUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cache_read_tokens: u.cache_read_tokens,
        cache_write_tokens: u.cache_write_tokens,
        cache_write_1h_tokens: u.cache_write_1h_tokens,
    });

    let empty_reason = resp
        .empty_reason
        // An empty string is no claim, not a claim of an unnamed reason.
        // Everything downstream — storage, the wire projection, the summary
        // tally — treats `""` as the "known: not empty" sentinel, so letting it
        // through here would create the one document those readers disagree on.
        .filter(|reason| !reason.is_empty())
        .map(EmptyReason::new)
        // Derive only when the output really is blank. A child reporting
        // `stop_reason: "max_tokens"` next to a complete answer hit the ceiling
        // *after* saying something useful; labelling that "truncated" would
        // send a reader after the wrong fix.
        //
        // The child's own claim always wins, and is honoured even on a
        // non-blank output: `tool_use_only` alongside a structured tool call is
        // a legitimate combination, and the child is the authority on its own
        // response.
        .or_else(|| {
            crate::empty::classify_blank(&output)?;
            resp.stop_reason
                .as_deref()
                .and_then(crate::empty::from_stop_reason)
        });

    Ok(ProviderResponse {
        output,
        usage,
        cost_usd: resp.cost_usd,
        stop_reason: resp.stop_reason,
        reasoning: resp.reasoning,
        empty_reason,
        model: resp.model,
        raw: resp.metadata,
        tool_calls: resp
            .tool_calls
            .into_iter()
            .map(|c| domarinn_types::result::ToolCall {
                id: c.id,
                name: c.name,
                arguments: c.arguments,
            })
            .collect(),
    })
}

#[cfg(test)]
#[path = "exec_provider_tests.rs"]
mod tests;

#[cfg(test)]
mod tool_tests {
    use super::*;
    use crate::provider::TestMeta;
    use serde_json::json;

    fn tool_req(tools: Vec<crate::config::ToolDef>) -> ProviderRequest {
        ProviderRequest {
            tools,
            prompt: None,
            vars: Default::default(),
            params: Default::default(),
            test: TestMeta {
                id: "t".into(),
                tags: vec![],
            },
            case_salt: None,
        }
    }

    /// A suite that declares no tools must produce the request it always did —
    /// otherwise every child parsing the wire format sees a new key, and every
    /// cached entry keyed on the request is invalidated for nothing.
    #[test]
    fn a_suite_without_tools_sends_no_tools_key() {
        let wire = serde_json::to_value(protocol_request(&tool_req(vec![]))).unwrap();
        assert!(wire.get("tools").is_none(), "{wire}");
    }

    #[test]
    fn declared_tools_reach_the_child() {
        let wire =
            serde_json::to_value(protocol_request(&tool_req(vec![crate::config::ToolDef {
                name: "get_weather".into(),
                description: Some("look up the weather".into()),
                input_schema: Some(json!({"type": "object"})),
            }])))
            .unwrap();
        assert_eq!(wire["tools"][0]["name"], "get_weather");
        assert_eq!(wire["tools"][0]["input_schema"]["type"], "object");
    }

    /// The end the assertions grade. A child that reports a call alongside
    /// `tool_use_only` is stating that the answer *is* the call — a case that
    /// previously had no gradeable output at all.
    #[test]
    fn a_child_can_report_structured_tool_calls() {
        let parsed = parse_response(json!({
            "output": "",
            "empty_reason": "tool_use_only",
            "tool_calls": [
                {"id": "c1", "name": "get_weather", "arguments": {"city": "Oslo"}}
            ]
        }))
        .unwrap();
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].name, "get_weather");
        assert_eq!(parsed.tool_calls[0].arguments["city"], "Oslo");
        // The child's own claim about why the output is empty still wins, even
        // though the output is blank and could have been derived.
        assert_eq!(
            parsed.empty_reason.as_ref().map(|r| r.as_str()),
            Some("tool_use_only")
        );
    }
}
