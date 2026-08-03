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
    /// Tools the suite declared, offered to the model by whichever provider
    /// knows how. Part of the *request*, so it is in the cache key: a call with
    /// tools and one without are different questions.
    pub tools: Vec<crate::config::ToolDef>,
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
    /// The tool calls the model decided to make, in order. Empty when it made
    /// none, or when the provider cannot report them.
    pub tool_calls: Vec<domarinn_types::result::ToolCall>,
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
            tool_calls: Vec::new(),
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

/// Build the [`Provider::canonical_request`] envelope for a provider that
/// appends a known path to a configurable `base_url`.
///
/// `path` rather than the full `url` the preview shows, because `base_url` is
/// **not** part of what makes two calls interchangeable. A gateway and a direct
/// connection to the same API answer the same question, and keying the host
/// would make them pay for the same answers twice — for the same reason
/// [`crate::cache_key`] refuses to key the model a provider *reports*, where a
/// vendor rolling a snapshot must not discard every entry at once.
///
/// The trade that buys is the same one [`Provider::program_digest`] makes: the
/// `base_url` is recorded on the cache entry and *reported* when a hit came from
/// a different one, so a local stub standing in for a vendor is visible rather
/// than silently replayed. A suite that needs them separated outright sets a
/// `cache_salt`.
///
/// Headers are absent for the same reason they are absent from the preview: they
/// carry the credential. A provider with headers worth keying publishes a digest
/// of them instead.
/// One vendor call's address, in both the form it is sent and the form it is
/// keyed.
///
/// Returned by the two request builders that do not go through the [`Provider`]
/// trait — the embeddings client and the `llm-rubric` judge — because they need
/// to hand their caller both: the full `url` to post to, and the `path` alone to
/// key on. Keeping them in one value is what stops a caller posting to one
/// address and keying another.
#[derive(Debug, Clone, PartialEq)]
pub struct VendorCall {
    /// The full URL, `base_url` included. What goes on the wire.
    pub url: String,
    /// The path and query alone. What the cache keys — see
    /// [`http_canonical_request`].
    pub path: String,
    pub body: Json,
}

pub fn http_canonical_request(method: &str, path: &str, body: Json) -> Json {
    serde_json::json!({
        "transport": "http",
        "method": method,
        "path": path,
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

    /// What *selects* this provider: canonical JSON naming the thing that will
    /// answer — type, plus `model`/`base_url`/`command`/`url` and any
    /// `cache_salt`.
    ///
    /// **Frozen.** It was the identity half of every cache key through 0.4.x;
    /// since 0.5.0 the key hashes the canonical request instead
    /// ([`Provider::canonical_request`]). Two things still read it, and both
    /// break if it moves: `digests::provider_digest`, which `--against` run
    /// diffing compares, and [`Provider::legacy_fingerprints`], which
    /// reconstructs ≤0.4.x keys to adopt entries under them. So a change here no
    /// longer invalidates a cache — it strands one.
    ///
    /// Two rules, both load-bearing:
    ///
    /// - **No secrets.** A fingerprint is persisted into every cache entry.
    ///   Where a credential genuinely separates two providers, publish a digest
    ///   of it rather than the value.
    /// - **Nothing machine-local.** No path, no `mtime`, no digest of a local
    ///   file, nothing read from the ambient environment. A fingerprint that
    ///   varies by machine cannot be shared, which quietly turns the S3 and
    ///   results-server backends into expensive local disk. See
    ///   [`crate::exec::program_digest`] for the case that taught this.
    fn fingerprint(&self) -> Json;

    async fn call(
        &self,
        req: &ProviderRequest,
        ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError>;

    /// Whether responses from this provider may be cached.
    ///
    /// Every built-in provider returns `true`. The hook survives for providers
    /// whose answers are inherently unrepeatable, and for embedders adding their
    /// own; `exec` used to return `false` without a `cache_salt`, which is the
    /// history [`crate::exec_provider::ExecProvider::cacheable`] records.
    fn cacheable(&self) -> bool {
        true
    }

    /// A digest of the local artifact that answered, when there is one.
    ///
    /// Stored on the cache entry and compared on a hit, so that a rebuilt
    /// program is *reported* rather than silently replayed. Deliberately not
    /// part of [`Provider::fingerprint`] — see [`crate::exec::program_digest`]
    /// for why paying the whole cache to detect a rebuild is the wrong trade.
    ///
    /// `None` for anything answering over a network, where the artifact is the
    /// vendor's and no digest of it exists.
    fn program_digest(&self) -> Option<&str> {
        None
    }

    /// The rate this provider's tokens are priced at, when it is priced at all.
    ///
    /// Exposed so a cache hit can be *re-costed* from the stored token counts
    /// rather than replaying a price that may be a year stale. Cost is not in
    /// the cache key — putting it there would discard every entry the day a
    /// vendor changes a price — so this is the same manoeuvre
    /// [`crate::cache::GradedVerdict`] makes with a grading `threshold`: store
    /// the raw measurement, apply the policy on read.
    ///
    /// `None` for a provider with no rate (an unknown model) and for `exec`,
    /// where the child reports a cost domarinn has no way to re-derive.
    fn rate(&self) -> Option<&crate::pricing::ModelRate> {
        None
    }

    /// Fingerprints this provider published in earlier versions, newest first.
    ///
    /// Consulted only on a miss, so entries keyed the ≤0.4.x way — a hash of the
    /// fingerprint plus the request pieces — can be adopted instead of re-paid
    /// for. See [`crate::cache_migrate`], which owns the historical literals,
    /// the key function that consumes these, and the deletion timeline.
    ///
    /// The default is no longer empty: 0.5.0 moved the live key off the
    /// fingerprint entirely, so *every* provider has exactly one shape to
    /// migrate from even if its fingerprint never changed — its own current one.
    /// Overriding this means prepending, never replacing: a provider whose
    /// fingerprint has older generations returns
    /// `[self.fingerprint(), …older…]`.
    ///
    /// Returned by value rather than as a slice because the ≤0.4.x shape is
    /// computed rather than stored. The cost is one clone per probing miss,
    /// which [`crate::cache_migrate::MigrationProbe`] caps at a handful of cases
    /// per run.
    fn legacy_fingerprints(&self) -> Vec<Json> {
        vec![self.fingerprint()]
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
    /// For what the *cache* keys on, see [`Provider::canonical_request`]: the
    /// same request, with whatever must not separate two callers withheld.
    ///
    /// `None` means "this provider does not describe its request", and the UI
    /// falls back to showing the rendered prompt alone.
    fn request_preview(&self, _req: &ProviderRequest) -> Option<Json> {
        None
    }

    /// The redacted canonical outgoing request: what [`Provider::call`] would
    /// send, with call-time env values replaced by stable placeholders and
    /// correlation metadata (the exec `test` block) stripped. This is the
    /// identity half of every cache key and is persisted verbatim into cache
    /// entries.
    ///
    /// Distinct from [`Provider::request_preview`], which exists to be read, and
    /// the two documents diverge for a different reason per provider. `openai`
    /// publishes the same envelope twice. `anthropic` *adds* a member here — its
    /// `anthropic-version` header, which selects a response shape and so must
    /// key, while a preview shows no headers at all. `exec` withholds: the
    /// `test` block is sent but stripped here, because a test's identity is not
    /// what makes two calls interchangeable. `http` renders both documents
    /// against placeholder `env` — its url/headers/body are templates resolved
    /// at call time, so there is no byte-faithful preview of it that could be
    /// persisted safely — and keys the `output_expr` it does not preview.
    ///
    /// `None` = this call is uncacheable.
    fn canonical_request(&self, _req: &ProviderRequest) -> Option<Json> {
        None
    }

    /// Where this provider's requests are addressed, when that is a fixed URL.
    ///
    /// Recorded on every cache entry and compared on a hit, so answers replayed
    /// from a different endpoint are *reported* rather than silently served.
    /// This is the whole compensation for keeping `base_url` out of the cache
    /// key — see [`http_canonical_request`] — and it is the same trade
    /// [`Provider::program_digest`] makes for an `exec` provider's bytes: the key
    /// stays portable, the change stays visible.
    ///
    /// `None` for a provider whose address is not fixed across a run (`http`
    /// templates its url per case, and it is in the canonical request anyway) or
    /// is not a URL at all (`exec`).
    fn address(&self) -> Option<String> {
        None
    }

    /// Canonical requests this provider published in earlier versions, newest
    /// first.
    ///
    /// The counterpart to [`Provider::legacy_fingerprints`] for the key space
    /// that replaced it. Consulted only on a miss, so entries written before a
    /// change to [`Provider::canonical_request`] can be adopted instead of
    /// re-paid for; the probe budget in [`crate::cache_adopt`] bounds what that
    /// costs a store with nothing to migrate.
    ///
    /// Empty by default, which is the honest answer for a provider whose
    /// canonical request has never changed. The three vendor providers override
    /// it because 0.8.0 took `base_url` out of theirs.
    ///
    /// Takes `req` because a canonical request is per-call, not per-provider —
    /// the reason this cannot be a stored `Vec<Json>` the way
    /// [`Provider::legacy_fingerprints`] nearly is.
    fn legacy_canonical_requests(&self, _req: &ProviderRequest) -> Vec<Json> {
        Vec::new()
    }

    /// The provider-level cache salt, when configured. Joins the cache key as
    /// its own member; never sent to the provider.
    fn cache_salt(&self) -> Option<&str> {
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
