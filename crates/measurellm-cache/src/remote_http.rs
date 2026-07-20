//! Cache backend that talks to the measurellm server's HTTP cache endpoints.
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
//! - `purge`→ `POST {base}/api/v1/cache/prune` (`?older_than_days=` when set),
//!   best-effort: any rejection or transport error yields `Ok(0)`.
//!
//! When a bearer token is configured it is sent as `Authorization: Bearer …` on
//! every request.

use async_trait::async_trait;
use measurellm_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use reqwest::{Client, RequestBuilder, StatusCode, Url};

/// A cache served by a remote measurellm server over HTTP.
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

    async fn purge(&self, filter: &PurgeFilter) -> Result<u64, CacheError> {
        // Best-effort: never surface an error to the caller. If the endpoint is
        // missing or rejects the request, report zero purged.
        let mut url = match self.endpoint(&["api", "v1", "cache", "prune"]) {
            Ok(url) => url,
            Err(_) => return Ok(0),
        };
        if let Some(older_than) = filter.older_than {
            url.query_pairs_mut()
                .append_pair("older_than_days", &older_than.num_days().to_string());
        }
        match self.authorize(self.client.post(url)).send().await {
            Ok(resp) if resp.status().is_success() => {
                // The server may report a count; fall back to 0 if it doesn't.
                #[derive(serde::Deserialize)]
                struct Pruned {
                    #[serde(default)]
                    pruned: u64,
                }
                Ok(resp.json::<Pruned>().await.map(|p| p.pruned).unwrap_or(0))
            }
            _ => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use measurellm_core::types::Output;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_entry() -> CacheEntry {
        CacheEntry {
            created_at: "2026-07-19T00:00:00Z".parse().unwrap(),
            provider_fingerprint: json!({"type": "exec"}),
            output: Output::Text("hi".into()),
            usage: None,
            cost_usd: None,
            stop_reason: None,
            measurellm_version: "test".into(),
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

    #[tokio::test]
    async fn purge_is_best_effort_on_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/cache/prune"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let cache = RemoteHttpCache::new(&server.uri(), None).unwrap();
        // Server rejects, but purge must not error.
        assert_eq!(cache.purge(&PurgeFilter::default()).await.unwrap(), 0);
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
        };
        assert_eq!(cache.purge(&filter).await.unwrap(), 3);
    }
}
