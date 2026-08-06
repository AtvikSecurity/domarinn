//! Cache backend that talks to the domarinn server's HTTP cache endpoints.
//!
//! Wire contract (all keys are a single `sha256:<hex>` path segment):
//!
//! - `get`  → `GET  {base}/api/v1/cache/{key}`
//!   - `200` → body is the raw JSON of a [`CacheEntry`] (deserialized)
//!   - `404` → `Ok(None)`
//!   - other → error
//! - `put`  → `PUT  {base}/api/v1/cache/{key}` with body `serde_json::to_vec(entry)`
//!   - `2xx` (server returns `200`/`201`) → `Ok(())`
//!   - other → error
//! - `stats`→ `GET  {base}/api/v1/cache/stats` → JSON [`CacheStats`]
//! - `purge`→ `POST {base}/api/v1/cache/prune`, with every predicate the filter
//!   names forwarded as a query param (see [`RemoteHttpCache::purge`])
//!   - `2xx` → the server's `{"pruned": N}` count, or `0` if it reports none
//!   - other → error, including the status
//!
//! When a bearer token is configured it is sent as `Authorization: Bearer …` on
//! every request.

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use reqwest::{Client, RequestBuilder, StatusCode, Url};

/// A cache served by a remote domarinn server over HTTP.
pub struct RemoteHttpCache {
    client: Client,
    base: Url,
    token: Option<String>,
}

impl RemoteHttpCache {
    /// Create a client for `base_url` (e.g. `http://host:8321`) with an optional
    /// bearer token.
    pub fn new(base_url: &str, token: Option<String>) -> Result<Self, CacheError> {
        let client = Client::builder()
            .build()
            .map_err(|e| CacheError(anyhow::anyhow!("building http client: {e}")))?;
        Self::with_client(client, base_url, token)
    }

    /// Create from an explicit [`reqwest::Client`] (useful for tests / custom TLS).
    pub fn with_client(
        client: Client,
        base_url: &str,
        token: Option<String>,
    ) -> Result<Self, CacheError> {
        let base = Url::parse(base_url)
            .map_err(|e| CacheError(anyhow::anyhow!("invalid base url {base_url:?}: {e}")))?;
        Ok(RemoteHttpCache {
            client,
            base,
            token,
        })
    }

    /// Build `{base}/<segments...>`, percent-encoding each segment defensively.
    ///
    /// The key segment `sha256:<hex>` contains no slashes, so it stays a single
    /// path segment; the URL layer leaves the `:` intact (it is a legal path
    /// char) while still escaping anything unexpected.
    fn endpoint(&self, segments: &[&str]) -> Result<Url, CacheError> {
        let mut url = self.base.clone();
        {
            let mut path = url.path_segments_mut().map_err(|_| {
                CacheError(anyhow::anyhow!("base url cannot be a base: {}", self.base))
            })?;
            // Drop a trailing empty segment (from a base like `http://h/`) so we
            // don't produce `//api`.
            path.pop_if_empty();
            path.extend(segments.iter().copied());
        }
        Ok(url)
    }

    /// Attach the bearer token to a request when one is configured.
    fn authorize(&self, rb: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(token) => rb.bearer_auth(token),
            None => rb,
        }
    }
}

fn transport_err(context: &str, e: reqwest::Error) -> CacheError {
    CacheError(anyhow::anyhow!("{context}: {e}"))
}

#[async_trait]
impl CacheBackend for RemoteHttpCache {
    #[tracing::instrument(level = "debug", skip(self), fields(key = %key))]
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        let url = self.endpoint(&["api", "v1", "cache", &key.0])?;
        let resp = self
            .authorize(self.client.get(url))
            .send()
            .await
            .map_err(|e| transport_err("cache get request", e))?;
        match resp.status() {
            StatusCode::OK => {
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| transport_err("reading cache get body", e))?;
                let entry = serde_json::from_slice(&bytes).map_err(|e| {
                    CacheError(anyhow::anyhow!("decoding cache entry for {}: {e}", key.0))
                })?;
                Ok(Some(entry))
            }
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(CacheError(anyhow::anyhow!(
                "cache get {}: unexpected status {status}",
                key.0
            ))),
        }
    }

    #[tracing::instrument(level = "debug", skip(self, entry), fields(key = %key))]
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        let url = self.endpoint(&["api", "v1", "cache", &key.0])?;
        let body = serde_json::to_vec(entry)
            .map_err(|e| CacheError(anyhow::anyhow!("serializing entry: {e}")))?;
        let resp = self
            .authorize(self.client.put(url).body(body))
            .send()
            .await
            .map_err(|e| transport_err("cache put request", e))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(CacheError(anyhow::anyhow!(
                "cache put {}: unexpected status {}",
                key.0,
                resp.status()
            )))
        }
    }

    async fn stats(&self) -> Result<CacheStats, CacheError> {
        let url = self.endpoint(&["api", "v1", "cache", "stats"])?;
        let resp = self
            .authorize(self.client.get(url))
            .send()
            .await
            .map_err(|e| transport_err("cache stats request", e))?;
        if !resp.status().is_success() {
            return Err(CacheError(anyhow::anyhow!(
                "cache stats: unexpected status {}",
                resp.status()
            )));
        }
        resp.json::<CacheStats>()
            .await
            .map_err(|e| transport_err("decoding cache stats", e))
    }

    /// Ask the server to prune, forwarding every predicate as a query param.
    ///
    /// ## Why a non-success status is an error and not `Ok(0)`
    ///
    /// This used to end in `_ => Ok(0)`, which reported "removed 0 entries" for
    /// a `401` (the prune route is admin-scoped) and for a `400` (an older
    /// server whose `PruneQuery` is `deny_unknown_fields` rejecting a param
    /// this build invented). Both are cases where the operator's request did
    /// not happen, and the `400` is worse than it looks: a bare POST with no
    /// params means "apply the server's full configured retention", so a
    /// filter silently degrading to that is not a no-op but a *wider* deletion
    /// than the one asked for. Nothing here retries without the params for
    /// exactly that reason.
    ///
    /// ## Wire contract
    ///
    /// | param | form |
    /// |---|---|
    /// | `older_than_days` | i64 |
    /// | `newer_than_days` | i64 |
    /// | `empty_reason` | comma-joined, e.g. `refusal,content_filter` |
    /// | `model` | string |
    /// | `kind` | string |
    ///
    /// `empty_reason` is one comma-joined value rather than a repeated key
    /// because the server deserializes it as `Option<String>`: a repeated key
    /// would silently keep whichever one the deserializer saw last, so half the
    /// operator's reasons would vanish without a word.
    ///
    /// Pre-existing and deliberately left alone here: `num_days()` truncates
    /// toward zero, so `--older-than 12h` crosses the wire as
    /// `older_than_days=0`. On the server that means "everything", which is
    /// dramatically more than 12h asked for. Sub-day windows against a remote
    /// tier are not yet expressible; fixing it needs a param the server does
    /// not have.
    ///
    /// The same truncation makes `newer_than` unusable below a day, in the
    /// other direction: `--older-than 30d --newer-than 12h` arrives as
    /// `older_than_days=30&newer_than_days=0`, i.e. `created_at < now-30d AND
    /// created_at >= now`, which matches nothing. The disk tier honours the real
    /// bound, so the two tiers disagree for sub-day windows. Worth knowing
    /// before reading a `pruned: 0` as "there was nothing to remove".
    async fn purge(&self, filter: &PurgeFilter) -> Result<u64, CacheError> {
        let mut url = self.endpoint(&["api", "v1", "cache", "prune"])?;
        // Only a default filter leaves the URL untouched — `cache clear`, which
        // is the one call that legitimately means "apply the server's full
        // configured retention".
        if !filter.is_default() {
            {
                let mut q = url.query_pairs_mut();
                if let Some(older_than) = filter.older_than {
                    q.append_pair("older_than_days", &older_than.num_days().to_string());
                }
                if let Some(newer_than) = filter.newer_than {
                    q.append_pair("newer_than_days", &newer_than.num_days().to_string());
                }
                if !filter.empty_reason.is_empty() {
                    let joined = filter
                        .empty_reason
                        .iter()
                        .map(|r| r.as_str())
                        .collect::<Vec<_>>()
                        .join(",");
                    q.append_pair("empty_reason", &joined);
                }
                if let Some(model) = &filter.model {
                    q.append_pair("model", model);
                }
                if let Some(kind) = &filter.kind {
                    q.append_pair("kind", kind);
                }
            }

            // Defensive rather than reachable today, and worth the four lines
            // anyway: a predicate added above without a matching `append_pair`
            // would otherwise turn a narrow eviction into the *widest* prune the
            // server offers, silently and with a success exit code.
            if url.query().unwrap_or("").is_empty() {
                return Err(CacheError(anyhow::anyhow!(
                    "cache prune: filter named a predicate but produced no query \
                     params; refusing to send a bare prune, which would apply the \
                     server's full retention policy instead"
                )));
            }
        }

        let resp = self
            .authorize(self.client.post(url))
            .send()
            .await
            .map_err(|e| transport_err("cache prune request", e))?;
        if !resp.status().is_success() {
            return Err(CacheError(anyhow::anyhow!(
                "cache prune: unexpected status {}",
                resp.status()
            )));
        }
        // The server may report a count; fall back to 0 if it doesn't.
        #[derive(serde::Deserialize)]
        struct Pruned {
            #[serde(default)]
            pruned: u64,
        }
        Ok(resp.json::<Pruned>().await.map(|p| p.pruned).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domarinn_core::types::Output;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_entry() -> CacheEntry {
        CacheEntry {
            kind: None,
            tool_calls: Vec::new(),
            created_at: "2026-07-19T00:00:00Z".parse().unwrap(),
            provider_fingerprint: Some(json!({"type": "exec"})),
            request: None,
            output: Output::Text("hi".into()),
            usage: None,
            cost_usd: None,
            stop_reason: None,
            model: None,
            verdict: None,
            raw: None,
            reasoning: None,
            empty_reason: None,
            attempts: None,
            provider_latency_ms: None,
            program_digest: None,
            address: None,
            domarinn_version: "test".into(),
        }
    }

    fn key() -> CacheKey {
        CacheKey::compute(&json!({"a": 1}))
    }

    #[tokio::test]
    async fn get_returns_none_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/cache/{}", key().0)))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        assert!(cache.get(&key()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_deserializes_entry_on_200() {
        let server = MockServer::start().await;
        let body = serde_json::to_vec(&sample_entry()).unwrap();
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/cache/{}", key().0)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        let got = cache.get(&key()).await.unwrap().unwrap();
        assert_eq!(got.output, Output::Text("hi".into()));
    }

    #[tokio::test]
    async fn put_sends_serialized_body_and_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(format!("/api/v1/cache/{}", key().0)))
            .and(body_json(sample_entry()))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        cache.put(&key(), &sample_entry()).await.unwrap();
        // MockServer verifies the `.expect(1)` on drop.
    }

    #[tokio::test]
    async fn sends_bearer_token_when_configured() {
        let server = MockServer::start().await;
        let body = serde_json::to_vec(&sample_entry()).unwrap();
        // Only matches when the Authorization header is exactly right.
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/cache/{}", key().0)))
            .and(header("authorization", "Bearer secret-token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .expect(1)
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), Some("secret-token".into())).unwrap();
        let got = cache.get(&key()).await.unwrap();
        assert!(got.is_some(), "request should have matched the bearer mock");
    }

    #[tokio::test]
    async fn get_errors_on_unexpected_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/cache/{}", key().0)))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        assert!(cache.get(&key()).await.is_err());
    }

    #[tokio::test]
    async fn stats_parses_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/cache/stats"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"entries": 7, "total_bytes": 1024})),
            )
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.entries, 7);
        assert_eq!(stats.total_bytes, 1024);
    }

    /// A prune the server refused is not a prune that removed nothing. The
    /// statuses that land here in practice are `401` (the route is
    /// admin-scoped) and `400` (an older server rejecting a param this build
    /// invented) — and reporting either as "removed 0 entries" tells the
    /// operator their eviction worked when the poison is still there.
    #[tokio::test]
    async fn a_non_success_status_is_surfaced_not_swallowed() {
        for status in [400u16, 401, 404, 500] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/api/v1/cache/prune"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;

            let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
            let err = cache
                .purge(&PurgeFilter::default())
                .await
                .expect_err("status {status} must not be reported as a successful prune");
            assert!(
                err.to_string().contains(&status.to_string()),
                "the message must name the status: {err}"
            );
        }
    }

    #[tokio::test]
    async fn purge_maps_older_than_to_days_and_reads_count() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/cache/prune"))
            .and(wiremock::matchers::query_param("older_than_days", "7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"pruned": 3})))
            .expect(1)
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        let filter = PurgeFilter {
            older_than: Some(chrono::Duration::days(7)),
            ..Default::default()
        };
        assert_eq!(cache.purge(&filter).await.unwrap(), 3);
    }

    /// The whole wire contract in one mock. `empty_reason` is asserted in its
    /// comma-joined form specifically: the server takes it as `Option<String>`,
    /// so sending a repeated key would keep one reason and drop the rest
    /// without complaint.
    #[tokio::test]
    async fn purge_forwards_every_filter_as_a_query_param() {
        use wiremock::matchers::query_param;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/cache/prune"))
            .and(query_param("older_than_days", "30"))
            .and(query_param("newer_than_days", "90"))
            .and(query_param("empty_reason", "refusal,content_filter"))
            .and(query_param("model", "claude-sonnet-4"))
            .and(query_param("kind", "provider"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"pruned": 12})))
            .expect(1)
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        let filter = PurgeFilter {
            older_than: Some(chrono::Duration::days(30)),
            newer_than: Some(chrono::Duration::days(90)),
            empty_reason: vec![
                domarinn_core::empty::EmptyReason::new(domarinn_core::empty::EmptyReason::REFUSAL),
                domarinn_core::empty::EmptyReason::new(
                    domarinn_core::empty::EmptyReason::CONTENT_FILTER,
                ),
            ],
            model: Some("claude-sonnet-4".into()),
            kind: Some("provider".into()),
        };
        assert_eq!(cache.purge(&filter).await.unwrap(), 12);
    }

    /// `cache clear` against a remote tier is still a bare POST — the one call
    /// that legitimately carries no params, because "no predicate" is what the
    /// server reads as its full configured retention.
    #[tokio::test]
    async fn a_default_filter_sends_no_query_params() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/cache/prune"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"pruned": 5})))
            .expect(1)
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        assert_eq!(cache.purge(&PurgeFilter::default()).await.unwrap(), 5);
    }
}
