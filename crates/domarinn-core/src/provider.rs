//! The [`Provider`] trait — the seam between the runner and a system under test.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::empty::EmptyReason;
use crate::error_class::ErrorClass;
use crate::types::{Output, RenderedPrompt, TokenUsage};

/// Metadata about the test a call belongs to (exec providers receive this).
#[derive(Debug, Clone, Default)]
pub struct TestMeta {
    pub id: String,
    pub tags: Vec<String>,
}

/// A single request to a provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderRequest {
    /// The rendered prompt, or `None` when the provider builds its own input.
    pub prompt: Option<RenderedPrompt>,
    /// Rendered test variables.
    pub vars: BTreeMap<String, Json>,
    /// Per-call parameter overrides, merged over the provider's own params.
    pub params: serde_json::Map<String, Json>,
    pub test: TestMeta,
    /// The case's opaque cache salt ([`crate::config::TestCase::cache_salt`]).
    /// Enters the cache key only when present, and never reaches the provider —
    /// it exists so a suite can bust one case's entry when content domarinn
    /// cannot see (the system under test's own prompt files) changes.
    pub case_salt: Option<String>,
}

/// A provider's response.
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub output: Output,
    pub usage: Option<TokenUsage>,
    pub cost_usd: Option<f64>,
    pub stop_reason: Option<String>,
    /// Raw payload, retained for `--verbose` / the UI.
    pub raw: Option<Json>,
    /// The model's reasoning/thinking text, when it exposed any.
    ///
    /// A separate field rather than an [`Output`] variant on purpose: `Output`
    /// is `#[serde(untagged)]`, so anything structured added as a variant would
    /// be silently swallowed by `Output::Json`.
    pub reasoning: Option<String>,
    /// Why [`Self::output`] has nothing gradeable in it, when it does not.
    pub empty_reason: Option<EmptyReason>,
    /// The model the provider actually used, as opposed to the one it was
    /// asked for — an alias that silently repoints to a new snapshot has no
    /// other signal, and a suite can pin a model that quietly stopped being
    /// the model it names.
    ///
    /// Response metadata, not request identity: this never enters a cache key.
    /// See `cache_key.rs` for why that is structural rather than a choice.
    pub model: Option<String>,
}

impl ProviderResponse {
    pub fn text(output: impl Into<String>) -> Self {
        ProviderResponse {
            output: Output::Text(output.into()),
            usage: None,
            cost_usd: None,
            stop_reason: None,
            raw: None,
            reasoning: None,
            model: None,
            empty_reason: None,
        }
    }
}

/// Provider failures, split by whether a retry could help.
///
/// Both variants carry an [`ErrorClass`], set at the point of failure. That
/// placement is the point: `net.rs` collapses 429, 5xx, timeout and 4xx into a
/// variant plus a prose string, so by the time the runner builds a
/// `CaseResult` the status code exists *only* inside the display text.
/// Re-deriving the class by sniffing `"HTTP 429"` back out would couple
/// classification to a message format, which is exactly what it exists to
/// escape.
///
/// The variant and the class are independent axes, not two names for one thing:
/// a `Retry-After` longer than the retry budget becomes `Fatal`, and its class
/// is still `provider_rate_limit`.
///
/// `details` is the structured half of a failure, alongside the prose in
/// `source`. A provider that knows something specific — which model it asked
/// for, what the endpoint said, how far a completion got — has somewhere to put
/// it that survives to the stored case, instead of formatting JSON into a
/// sentence and hoping the reader parses it back out.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// Transient — 429/5xx/timeout/connection. May carry a server `Retry-After`.
    #[error("retriable provider error: {source}")]
    Retriable {
        #[source]
        source: anyhow::Error,
        retry_after: Option<Duration>,
        class: ErrorClass,
        details: Option<Json>,
    },
    /// Permanent — 4xx, bad protocol, misconfiguration.
    #[error("fatal provider error: {source}")]
    Fatal {
        #[source]
        source: anyhow::Error,
        class: ErrorClass,
        details: Option<Json>,
    },
}

impl ProviderError {
    /// A permanent failure. `class` is one of the [`ErrorClass`] constants.
    pub fn fatal(class: &str, source: anyhow::Error) -> Self {
        ProviderError::Fatal {
            source,
            class: ErrorClass::new(class),
            details: None,
        }
    }

    /// A transient failure worth retrying, optionally honouring a server
    /// `Retry-After`.
    pub fn retriable(class: &str, source: anyhow::Error, retry_after: Option<Duration>) -> Self {
        ProviderError::Retriable {
            source,
            retry_after,
            class: ErrorClass::new(class),
            details: None,
        }
    }

    /// Attach structured diagnostics. Chained onto a constructor rather than
    /// being a fourth positional argument, because the overwhelming majority of
    /// failure sites have nothing structured to say and should stay short.
    pub fn with_details(mut self, value: Option<Json>) -> Self {
        match &mut self {
            ProviderError::Retriable { details, .. } | ProviderError::Fatal { details, .. } => {
                *details = value
            }
        }
        self
    }

    pub fn class(&self) -> &ErrorClass {
        match self {
            ProviderError::Retriable { class, .. } | ProviderError::Fatal { class, .. } => class,
        }
    }

    pub fn details(&self) -> Option<&Json> {
        match self {
            ProviderError::Retriable { details, .. } | ProviderError::Fatal { details, .. } => {
                details.as_ref()
            }
        }
    }
}

/// Build the [`Provider::request_preview`] envelope for an HTTP-style provider.
///
/// `body` is the verbatim JSON payload — the thing a developer can lift straight
/// into `curl`. Headers are deliberately absent: they carry the API key.
pub fn http_request_preview(method: &str, url: &str, body: Json) -> Json {
    serde_json::json!({
        "transport": "http",
        "method": method,
        "url": url,
        "body": body,
    })
}

/// Build the [`Provider::request_preview`] envelope for a subprocess provider.
///
/// `stdin` is the provider-protocol document written to the child's stdin.
pub fn exec_request_preview(command: &str, args: &[String], stdin: Json) -> Json {
    serde_json::json!({
        "transport": "exec",
        "command": command,
        "args": args,
        "stdin": stdin,
    })
}

/// Ambient context passed to a provider call (secrets, cwd, cancellation, etc.).
#[derive(Debug, Clone, Default)]
pub struct CallCtx {
    /// Directory to resolve relative commands/paths against (the config dir).
    pub working_dir: Option<std::path::PathBuf>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable id from the config.
    fn id(&self) -> &str;

    /// A stable identity used in cache keys. Canonical JSON of the provider's
    /// behavior (type, model/command/url, params, and any `cache_salt`).
    /// Must exclude secrets.
    fn fingerprint(&self) -> Json;

    async fn call(
        &self,
        req: &ProviderRequest,
        ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError>;

    /// Whether responses from this provider may be cached. Defaults to true;
    /// exec providers return false unless a `cache_salt` pins the version of the
    /// system under test, so a rebuilt binary is never served stale output.
    fn cacheable(&self) -> bool {
        true
    }

    /// The request this provider *would* send for `req` — the payload the model
    /// actually receives, not a re-description of it.
    ///
    /// This exists because the wire body is the one thing the UI could not
    /// honestly reconstruct: it is assembled per provider (the OpenAI shape
    /// merges `params` and folds a text prompt into a single user message;
    /// Anthropic lifts `system` out of the message list; the HTTP provider
    /// renders a caller-authored template), so any client-side guess would be
    /// wrong for three of the four providers while looking authoritative.
    ///
    /// Built from the same code path as [`Provider::call`] so the two cannot
    /// drift, and pure, so a cache hit — where no HTTP request is made at all —
    /// still reports the request the cached entry stands for. Must exclude
    /// secrets: bodies only, never headers.
    ///
    /// `None` means "this provider does not describe its request", and the UI
    /// falls back to showing the rendered prompt alone.
    fn request_preview(&self, _req: &ProviderRequest) -> Option<Json> {
        None
    }

    /// Why this response has nothing gradeable in it, if it does not.
    ///
    /// A trait method rather than logic inlined in each parse function so a new
    /// provider — most plausibly an `exec` child rather than a new Rust module —
    /// gets the provider-agnostic baseline for free and can override it with
    /// vendor specifics. Sits alongside [`Provider::cacheable`] and
    /// [`Provider::request_preview`], the other per-provider policy hooks.
    ///
    /// Providers that can say more should set
    /// [`ProviderResponse::empty_reason`] at parse time, while the evidence
    /// (finish reasons, block types) is still in hand; this default only runs
    /// when they did not.
    fn classify_empty(&self, response: &ProviderResponse) -> Option<EmptyReason> {
        response
            .empty_reason
            .clone()
            .or_else(|| crate::empty::classify_blank(&response.output))
    }
}
