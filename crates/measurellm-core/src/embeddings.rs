//! Embeddings provider (OpenAI-compatible `/embeddings`) and cosine similarity,
//! used by the `similar` assertion.

use std::time::Duration;

use serde_json::{json, Value as Json};

use crate::config::ParamMap;
use crate::net::{api_key, http_client};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// An OpenAI-compatible embeddings client.
pub struct EmbeddingsProvider {
    model: String,
    base_url: String,
    api_key_env: String,
    params: ParamMap,
    client: reqwest::Client,
}

impl EmbeddingsProvider {
    pub fn new(
        model: impl Into<String>,
        base_url: Option<String>,
        api_key_env: Option<String>,
        params: Option<ParamMap>,
    ) -> Self {
        EmbeddingsProvider {
            model: model.into(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key_env: api_key_env.unwrap_or_else(|| "OPENAI_API_KEY".to_string()),
            params: params.unwrap_or_default(),
            client: http_client(DEFAULT_TIMEOUT),
        }
    }

    /// Embed a single string, returning its vector.
    pub async fn embed(&self, text: &str) -> Result<Vec<f64>, String> {
        let key = api_key(&self.api_key_env).map_err(|e| e.to_string())?;
        let mut body = serde_json::Map::new();
        for (k, v) in &self.params {
            body.insert(k.clone(), v.clone());
        }
        body.insert("model".into(), json!(self.model));
        body.insert("input".into(), json!(text));
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));

        let resp = self
            .client
            .post(&url)
            .bearer_auth(key)
            .json(&Json::Object(body))
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
        let payload: Json = resp.json().await.map_err(|e| e.to_string())?;
        payload
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|d| d.first())
            .and_then(|e| e.get("embedding"))
            .and_then(|e| e.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "embeddings response missing data[0].embedding".to_string())
    }
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
}
