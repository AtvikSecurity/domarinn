//! Embeddings provider (OpenAI-compatible `/embeddings`) and cosine similarity,
//! used by the `similar` assertion.

use std::time::Duration;

use serde_json::{json, Value as Json};

use crate::config::ParamMap;
use crate::net::{api_key, http_client};
use crate::pricing::{MicroUsd, ModelRate};
use crate::types::TokenUsage;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// One embedding call's vector plus what it cost.
pub struct Embedded {
    pub vector: Vec<f64>,
    pub usage: Option<TokenUsage>,
    pub cost: Option<MicroUsd>,
}

/// An OpenAI-compatible embeddings client.
pub struct EmbeddingsProvider {
    model: String,
    base_url: String,
    api_key_env: crate::config::EnvNames,
    params: ParamMap,
    /// Resolved once at construction, so the unknown-model warning fires once
    /// per run rather than once per `similar` assertion — the same discipline
    /// the chat providers follow, and the reason neither needs global state.
    rate: Option<ModelRate>,
    client: reqwest::Client,
}

impl EmbeddingsProvider {
    pub fn new(
        provider_id: &str,
        model: impl Into<String>,
        base_url: Option<String>,
        api_key_env: Option<crate::config::EnvNames>,
        params: Option<ParamMap>,
        pricing: Option<&crate::config::PricingCfg>,
    ) -> Self {
        let model = model.into();
        EmbeddingsProvider {
            rate: crate::pricing::resolve_embedding_rate(provider_id, &model, pricing),
            model,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key_env: api_key_env.unwrap_or_else(|| "OPENAI_API_KEY".into()),
            params: params.unwrap_or_default(),
            client: http_client(DEFAULT_TIMEOUT),
        }
    }

    /// The url and body one embedding call would post.
    ///
    /// Pure: no credential, no clock. This is what the cache keys on (through
    /// [`crate::provider::http_request_preview`]), and it absorbs everything the
    /// deleted `identity` enumerated by hand — the model, the endpoint, the
    /// params — plus the one thing that fingerprint could never carry, the text
    /// being embedded. That is why a `similar` assertion now caches the *two
    /// embeddings* rather than only the cosine of them: a vector is reusable by
    /// any later comparison, a cosine is not.
    pub fn request(&self, text: &str) -> (String, Json) {
        let mut body = serde_json::Map::new();
        for (k, v) in &self.params {
            body.insert(k.clone(), v.clone());
        }
        body.insert("model".into(), json!(self.model));
        body.insert("input".into(), json!(text));
        (
            format!("{}/embeddings", self.base_url.trim_end_matches('/')),
            Json::Object(body),
        )
    }

    /// Post a built embedding request and return the raw payload.
    ///
    /// The only step that reads the credential, so a warm `similar` assertion
    /// never asks for one.
    pub async fn post(&self, url: &str, body: &Json) -> Result<Json, String> {
        let key = api_key(&self.api_key_env).map_err(|e| e.to_string())?;
        let resp = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {}: {}",
                resp.status().as_u16(),
                resp.text().await.unwrap_or_default()
            ));
        }
        resp.json().await.map_err(|e| e.to_string())
    }

    /// Read a vector and its bill out of an embeddings payload.
    ///
    /// Runs on a cache hit as well as a live call, so the cost is re-derived at
    /// today's rate rather than replayed from whatever was current when the
    /// vector was fetched.
    pub fn parse(&self, payload: &Json) -> Result<Embedded, String> {
        let vector = payload
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|d| d.first())
            .and_then(|e| e.get("embedding"))
            .and_then(|e| e.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "embeddings response missing data[0].embedding".to_string())?;
        let usage = usage_from_payload(payload);
        let cost = self
            .rate
            .as_ref()
            .zip(usage.as_ref())
            .and_then(|(rate, usage)| crate::pricing::cost_of(usage, rate));
        Ok(Embedded {
            vector,
            usage,
            cost,
        })
    }
}

/// The billable tokens in an embeddings response.
///
/// `prompt_tokens` only. The endpoint emits `total_tokens` as well, but for
/// embeddings the two are the same number, and adding both would double the
/// bill. There are no completion tokens and no cache counters to read.
fn usage_from_payload(payload: &Json) -> Option<TokenUsage> {
    let prompt_tokens = payload
        .get("usage")
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())?;
    Some(TokenUsage {
        input_tokens: prompt_tokens,
        output_tokens: 0,
        cache_read_tokens: None,
        cache_write_tokens: None,
        cache_write_1h_tokens: None,
    })
}

/// Cosine similarity of two vectors, in [-1, 1] (0 if either is empty or zero).
pub fn cosine(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        assert!((cosine(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-9);
    }

    #[test]
    fn cosine_mismatched_or_empty_is_zero() {
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
    }

    /// `total_tokens` equals `prompt_tokens` on this endpoint, so reading both
    /// would bill every embedding call twice.
    #[test]
    fn usage_counts_prompt_tokens_once() {
        let payload = json!({"usage": {"prompt_tokens": 42, "total_tokens": 42}});
        let usage = usage_from_payload(&payload).expect("usage parses");
        assert_eq!(usage.input_tokens, 42);
        assert_eq!(usage.output_tokens, 0);
    }

    /// An endpoint that reports no usage must produce no cost, not a zero one:
    /// zero is a claim that the call was free.
    #[test]
    fn a_response_without_usage_reports_none() {
        assert!(usage_from_payload(&json!({"data": []})).is_none());
    }

    /// The request is what the cache keys on, so swapping the embedding model
    /// must change it — otherwise a `similar` assertion replays vectors
    /// computed by a different model. The credential is not in it (it is a
    /// header), so naming a different env var must change nothing.
    ///
    /// This replaces the deleted `identity()`: the same claim, about the thing
    /// that is now actually keyed.
    #[test]
    fn the_request_moves_with_the_model_and_never_carries_the_key_env() {
        let one = EmbeddingsProvider::new("e", "text-embedding-3-small", None, None, None, None);
        let two = EmbeddingsProvider::new("e", "text-embedding-3-large", None, None, None, None);
        assert_ne!(one.request("hi"), two.request("hi"));

        let named = EmbeddingsProvider::new(
            "e",
            "text-embedding-3-small",
            None,
            Some("SECRET_KEY_VAR".into()),
            None,
            None,
        );
        assert_eq!(named.request("hi"), one.request("hi"));

        // …and the text is in it, which the old fingerprint could not express:
        // that is what makes a *vector* cacheable rather than only a cosine.
        assert_ne!(one.request("hi"), one.request("there"));
    }
}
