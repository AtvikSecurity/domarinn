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
    /// Identity of the program `command` runs, resolved once at construction so
    /// a cache lookup costs no filesystem access.
    program: Json,
    /// Digest of `env`, so two providers differing only in their environment do
    /// not share a key. A digest rather than the map itself because a
    /// fingerprint is persisted into the cache entry and `env` is a credential
    /// channel — see [`Self::request_preview`], which excludes it for the same
    /// reason.
    env_digest: Option<String>,
}

impl ExecProvider {
    /// `base_dir` is the directory children are spawned in — the suite's, in a
    /// real run. It is what a relative `command` is resolved against when the
    /// program's identity is computed, so passing `None` (tests, embedders with
    /// no suite on disk) means only absolute and `PATH` programs are identified.
    pub fn new(
        id: impl Into<String>,
        command: Vec<String>,
        env: BTreeMap<String, String>,
        timeout_ms: Option<u64>,
        cache_salt: Option<String>,
        base_dir: Option<&std::path::Path>,
    ) -> Self {
        let id = id.into();
        let program = crate::exec::program_identity(&command, base_dir);
        if !crate::exec::has_program_identity(&program) && cache_salt.is_none() {
            tracing::warn!(
                provider = %id,
                command = ?command,
                "could not identify the program behind this exec provider, so its \
                 responses will not be cached — a key over argv alone would replay \
                 stale output after a rebuild. Set `cache_salt` to cache anyway."
            );
        }
        ExecProvider {
            id,
            program,
            env_digest: env_digest(&env),
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

    fn fingerprint(&self) -> Json {
        serde_json::json!({
            "type": "exec",
            "command": self.command,
            // Computed once at construction. Without it the key is argv only,
            // which does not move when the program behind it is rebuilt — the
            // hazard that used to make exec caching opt-in.
            "program": self.program,
            // Two providers wrapping one script and differing only in `env` are
            // a normal A/B shape (`MODEL_ENDPOINT: http://a` vs `…/b`). Without
            // this they share a key and the second column silently replays the
            // first's answers, fabricating the comparison the run exists for.
            "env": self.env_digest,
            "cache_salt": self.cache_salt,
        })
    }

    /// Cacheable once the program behind `command` can be identified.
    ///
    /// This used to require a hand-managed `cache_salt`, because argv does not
    /// move when the binary behind it is rebuilt and a stale verdict in CI is
    /// worse than a cache miss. [`crate::exec::program_identity`] removes the
    /// need for the hand-management by putting the program's own identity in the
    /// fingerprint, so a rebuild busts the entry on its own.
    ///
    /// It removes the need only when it finds something. A command whose argv
    /// names nothing readable — `docker run …`, a wrapper invoked through an
    /// interpreter that is itself a shell builtin — has no identity beyond argv,
    /// and caching *that* is the original hazard rather than a fix for it. So
    /// the guard is the identity, not the calendar: found one, cache by default;
    /// found none, require the explicit `cache_salt` that says "I know what
    /// moves this."
    fn cacheable(&self) -> bool {
        self.cache_salt.is_some() || crate::exec::has_program_identity(&self.program)
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
mod tests {
    use super::*;
    use crate::provider::TestMeta;
    use serde_json::json;

    /// See the matching test in `anthropic.rs`: the fingerprint feeds every
    /// cache key, so an unconditional change invalidates every cached entry.
    #[test]
    fn fingerprint_is_stable_for_default_config() {
        let p = ExecProvider::new("p", vec!["./sut".into()], BTreeMap::new(), None, None, None);
        // `program` is empty here because `./sut` does not exist relative to the
        // `base_dir` passed (none), so nothing on disk was found to key on. The
        // asserted string changed twice, deliberately: once when `program` was
        // added to make exec caching safe by default, and once when `env` joined
        // it so two providers wrapping one script stop colliding. Each of those
        // invalidated every existing exec cache entry. Any *further* change does
        // the same, so treat a failure here as a cache migration to plan rather
        // than a test to update.
        assert_eq!(
            crate::cache::canonical_json(&p.fingerprint()),
            r#"{"cache_salt":null,"command":["./sut"],"env":null,"program":[],"type":"exec"}"#
        );
        // …and with nothing found, it declines to cache rather than keying on
        // argv alone.
        assert!(!p.cacheable());
    }

    /// The child is spawned with `cwd = base_dir`, so that is what a relative
    /// `command` must be resolved against. Statting it against the *process*
    /// cwd finds nothing, contributes nothing, and silently degrades the key
    /// back to argv — which is the whole hazard `program` exists to remove.
    #[test]
    fn a_relative_command_is_identified_against_the_suite_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sut"), "#!/bin/sh\necho v1").unwrap();

        let command = vec!["./sut".to_string()];
        let seen = ExecProvider::new("p", command.clone(), BTreeMap::new(), None, None, None);
        assert!(
            !seen.cacheable(),
            "sanity: with no base_dir there is nothing to find"
        );

        let p = ExecProvider::new("p", command, BTreeMap::new(), None, None, Some(dir.path()));
        assert!(p.cacheable());
        let fp = crate::cache::canonical_json(&p.fingerprint());
        assert!(fp.contains("\"len\""), "{fp}");
        // The argv string, not the resolved path: two machines sharing a cache
        // must not miss on each other for having checked the repo out elsewhere.
        assert!(!fp.contains(&dir.path().display().to_string()), "{fp}");
    }

    /// The most ordinary exec provider there is — a tool installed on `PATH` —
    /// and the one a rebuild moves. Before `PATH` was consulted this resolved to
    /// nothing, so every such provider was keyed on argv alone.
    #[test]
    fn a_path_resolved_program_is_identified() {
        let p = ExecProvider::new("p", vec!["sh".into()], BTreeMap::new(), None, None, None);
        assert!(p.cacheable());
        assert!(crate::cache::canonical_json(&p.fingerprint()).contains("\"len\""));
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

    /// A rebuild must bust the entry on its own, or caching exec by default
    /// would serve stale output — the hazard that used to make it opt-in.
    #[test]
    fn the_fingerprint_moves_when_the_program_changes() {
        let dir = tempfile::tempdir().unwrap();
        let prog = dir.path().join("sut");
        std::fs::write(&prog, "#!/bin/sh\necho v1").unwrap();
        let command = vec![prog.to_string_lossy().to_string()];

        let before = ExecProvider::new("p", command.clone(), BTreeMap::new(), None, None, None);
        let before_fp = crate::cache::canonical_json(&before.fingerprint());
        assert!(
            before_fp.contains("\"len\""),
            "an existing program must contribute its identity: {before_fp}"
        );

        // A rebuild: same argv, different bytes.
        std::fs::write(&prog, "#!/bin/sh\necho v2 and then some more").unwrap();
        let after = ExecProvider::new("p", command, BTreeMap::new(), None, None, None);
        assert_ne!(
            before_fp,
            crate::cache::canonical_json(&after.fingerprint())
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
        let without =
            ExecProvider::new("p", vec!["true".into()], BTreeMap::new(), None, None, None);
        assert!(with_salt.cacheable());
        assert!(without.cacheable());
        // The salt still separates them.
        assert_ne!(
            crate::cache::canonical_json(&with_salt.fingerprint()),
            crate::cache::canonical_json(&without.fingerprint())
        );
    }

    /// "By default" is not "unconditionally". A command whose argv names nothing
    /// readable has no identity beyond argv, and caching *that* is the original
    /// hazard rather than a fix for it — so the salt goes back to being required.
    #[tokio::test]
    async fn a_command_with_no_identifiable_program_needs_a_salt() {
        let unidentifiable = vec!["definitely-not-a-real-binary-xyz".to_string()];
        let bare = ExecProvider::new(
            "p",
            unidentifiable.clone(),
            BTreeMap::new(),
            None,
            None,
            None,
        );
        assert!(
            !bare.cacheable(),
            "argv alone must not be a cache key: a rebuild does not move it"
        );
        let salted = ExecProvider::new(
            "p",
            unidentifiable,
            BTreeMap::new(),
            None,
            Some("v1".into()),
            None,
        );
        assert!(salted.cacheable(), "the salt is the escape hatch");
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

    /// Open set: a reason this build has never heard of is carried verbatim,
    /// never rejected. Rejecting it would turn a diagnosis into a parse failure.
    #[test]
    fn an_unknown_empty_reason_is_carried_verbatim() {
        let resp =
            parse(serde_json::json!({"output": "", "empty_reason": "invented_later"})).unwrap();
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
            parse(serde_json::json!({"output": "a real answer", "stop_reason": "max_tokens"}))
                .unwrap();
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
}

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
