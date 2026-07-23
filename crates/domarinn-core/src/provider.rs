//! The [`Provider`] trait — the seam between the runner and a system under test.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as Json;

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
}

impl ProviderResponse {
    pub fn text(output: impl Into<String>) -> Self {
        ProviderResponse {
            output: Output::Text(output.into()),
            usage: None,
            cost_usd: None,
            stop_reason: None,
            raw: None,
        }
    }
}

/// Provider failures, split by whether a retry could help.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Transient — 429/5xx/timeout/connection. May carry a server `Retry-After`.
    #[error("retriable provider error: {source}")]
    Retriable {
        #[source]
        source: anyhow::Error,
        retry_after: Option<Duration>,
    },
    /// Permanent — 4xx, bad protocol, misconfiguration.
    #[error("fatal provider error: {0}")]
    Fatal(#[source] anyhow::Error),
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
}
