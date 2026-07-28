//! The [`CacheBackend`] trait and cache value types.
//!
//! Keys are content-addressed SHA-256 (`sha256:<64 hex>`), one entry per key,
//! immutable (first-write-wins). The trait lives in core; concrete backends
//! (disk, remote HTTP, S3, layered) live in `domarinn-cache`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use crate::types::{Output, TokenUsage};

/// A content-addressed cache key, string form `sha256:<64 lowercase hex>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheKey(pub String);

impl CacheKey {
    /// Compute a key from any canonically-serializable value.
    ///
    /// The input is serialized with sorted keys (via [`canonical_json`]) so map
    /// ordering never changes the key.
    pub fn compute(parts: &Json) -> CacheKey {
        let canonical = canonical_json(parts);
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        let digest = hasher.finalize();
        CacheKey(format!("sha256:{}", hex_lower(&digest)))
    }

    /// True when the string is a well-formed key.
    pub fn is_valid(s: &str) -> bool {
        s.strip_prefix("sha256:")
            .map(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
            .unwrap_or(false)
    }
}

impl std::fmt::Display for CacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// Serialize a JSON value canonically: object keys sorted, no insignificant
/// whitespace. This is what cache keys hash over, so it must be stable.
pub fn canonical_json(value: &Json) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => out.push_str(&n.to_string()),
        Json::String(s) => {
            // Reuse serde_json's string escaping for correctness.
            out.push_str(&serde_json::to_string(s).unwrap());
        }
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Json::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push(':');
                write_canonical(&map[k], out);
            }
            out.push('}');
        }
    }
}

/// A grading result *before* any threshold is applied.
///
/// The shape decision that makes verdict caching correct. `grade_llm_rubric`
/// used to fold `threshold` into `AssertOutcome.passed` before returning, and
/// caching that would have forced `threshold` into the cache key — so editing
/// a threshold would re-pay the judge for an answer it had already given.
///
/// Keeping the raw verdict makes "threshold is not in the key" *structural*
/// rather than a comment that can rot: it is not reachable from this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GradedVerdict {
    /// An `llm-rubric` judge's structured verdict.
    Rubric {
        score: f64,
        pass: bool,
        #[serde(default)]
        reasoning: String,
    },
    /// A `similar` assertion's cosine similarity, in `[-1, 1]`.
    Similarity { cosine: f64 },
    /// An `exec` assertion's protocol response.
    Exec {
        pass: bool,
        score: f64,
        #[serde(default)]
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Json>,
    },
}

impl GradedVerdict {
    /// Apply this assertion's threshold and shape the reason string.
    ///
    /// The only place a threshold is consulted, which is why changing one
    /// re-scores instantly from cache instead of re-grading.
    pub fn to_outcome(&self, threshold: Option<f64>) -> crate::assertion::AssertOutcome {
        use crate::assertion::AssertOutcome;
        match self {
            GradedVerdict::Rubric {
                score,
                pass,
                reasoning,
            } => AssertOutcome {
                score: *score,
                passed: match threshold {
                    Some(t) => *score >= t,
                    None => *pass,
                },
                reason: reasoning.clone(),
                details: None,
            },
            GradedVerdict::Similarity { cosine } => {
                let threshold = threshold.unwrap_or(0.8);
                AssertOutcome {
                    score: ((cosine + 1.0) / 2.0).clamp(0.0, 1.0),
                    passed: *cosine >= threshold,
                    reason: if *cosine >= threshold {
                        format!("cosine similarity {cosine:.3} >= {threshold:.3}")
                    } else {
                        format!("cosine similarity {cosine:.3} < {threshold:.3}")
                    },
                    details: None,
                }
            }
            GradedVerdict::Exec {
                pass,
                score,
                reason,
                details,
            } => AssertOutcome {
                score: *score,
                passed: *pass,
                reason: reason.clone(),
                details: details.clone(),
            },
        }
    }
}

/// A verdict plus what producing it cost.
///
/// Separate from [`GradedVerdict`] rather than folded into it, because only the
/// verdict decides an outcome: keeping cost out preserves the property that
/// nothing reachable from a cached answer can be keyed on a threshold. The cost
/// still rides along so a verdict cache entry replays it, exactly as a provider
/// entry replays its own `cost_usd` — a run's grading cost must not depend on
/// whether the verdicts came from cache.
#[derive(Debug, Clone, PartialEq)]
pub struct Graded {
    pub verdict: GradedVerdict,
    pub usage: Option<TokenUsage>,
    pub cost_usd: Option<f64>,
    /// The judge model that produced this verdict, as the API reported it —
    /// which is not necessarily the one configured, when an alias repoints.
    pub model: Option<String>,
}

impl Graded {
    /// A verdict with no cost attached: the `exec` path, where the child spends
    /// whatever it spends and domarinn has no way to see it.
    pub fn unpriced(verdict: GradedVerdict) -> Graded {
        Graded {
            verdict,
            usage: None,
            cost_usd: None,
            model: None,
        }
    }
}

/// A cached provider response, plus provenance for stats/debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub created_at: DateTime<Utc>,
    pub provider_fingerprint: Json,
    pub output: Output,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Raw provider metadata, replayed on a hit so cached cases keep their
    /// "Provider metadata" drawer section. Absent in entries written before
    /// this field existed (they replay with no raw, as they always did).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Json>,
    /// Attempts the original call took. Replayed so a cached case reports what
    /// actually happened rather than a sentinel — a hit used to report `0`,
    /// which rendered as the nonsense "0 attempts". Absent on entries written
    /// before this field existed; such a hit reports no attempt count at all,
    /// which is honest, where `0` was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    /// How long the original provider call took, excluding retry backoff.
    ///
    /// Without this a cache hit reports the *cache read* time as the model's
    /// latency — near-zero for a local store, but not for `remote_http` or
    /// `s3`, where it is simply a different measurement wearing the same name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_latency_ms: Option<u64>,
    /// The model the original call actually used.
    ///
    /// Mandatory to replay, not optional-in-spirit: without it a cache hit
    /// comes back with no model, so the field would be present on the first
    /// run of a suite and gone on every run after — which is the common path,
    /// not the rare one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Present only on grader-verdict entries.
    ///
    /// Its absence is exactly what "this is a provider response" means, so the
    /// two never need a discriminator beyond this field. A grader lookup that
    /// returns an entry without one is treated as a miss, never an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<GradedVerdict>,
    /// Reasoning/thinking text from the original call. Without this a cache hit
    /// replays with no reasoning and no explanation for an empty output — and a
    /// hit is the common path, so the diagnostic would be present on the first
    /// run and gone on every run after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_reason: Option<crate::empty::EmptyReason>,
    pub domarinn_version: String,
}

/// How the runner should interact with the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Normal read + write.
    ReadWrite,
    /// `--no-cache`: never read or write.
    Disabled,
    /// `--cache-only`: read only; a miss is an infrastructure error.
    ReadOnlyStrict,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStats {
    pub entries: u64,
    pub total_bytes: u64,
    #[serde(default)]
    pub hits: u64,
    #[serde(default)]
    pub misses: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_entry_at: Option<DateTime<Utc>>,
}

/// Filter for cache pruning.
#[derive(Debug, Clone, Default)]
pub struct PurgeFilter {
    pub older_than: Option<chrono::Duration>,
}

#[derive(Debug, thiserror::Error)]
#[error("cache error: {0}")]
pub struct CacheError(#[from] pub anyhow::Error);

#[async_trait]
pub trait CacheBackend: Send + Sync {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError>;
    /// Store an entry. Immutable: writing an existing key is a no-op.
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError>;
    async fn stats(&self) -> Result<CacheStats, CacheError>;
    async fn purge(&self, filter: &PurgeFilter) -> Result<u64, CacheError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_is_stable_regardless_of_map_order() {
        let a = CacheKey::compute(&json!({"b": 1, "a": 2}));
        let b = CacheKey::compute(&json!({"a": 2, "b": 1}));
        assert_eq!(a, b, "map ordering must not change the key");
    }

    #[test]
    fn key_changes_when_a_param_changes() {
        let a = CacheKey::compute(&json!({"model": "x", "max_tokens": 100}));
        let b = CacheKey::compute(&json!({"model": "x", "max_tokens": 200}));
        assert_ne!(a, b);
    }

    #[test]
    fn key_has_expected_shape() {
        let k = CacheKey::compute(&json!({"a": 1}));
        assert!(CacheKey::is_valid(&k.0), "{}", k.0);
    }

    #[test]
    fn rejects_malformed_keys() {
        assert!(!CacheKey::is_valid("sha256:XYZ"));
        assert!(!CacheKey::is_valid("md5:abc"));
        assert!(!CacheKey::is_valid("sha256:AABB")); // too short + uppercase
    }

    #[test]
    fn canonical_json_sorts_nested_keys() {
        let s = canonical_json(&json!({"z": {"y": 1, "x": 2}, "a": [3, 2]}));
        assert_eq!(s, r#"{"a":[3,2],"z":{"x":2,"y":1}}"#);
    }
}
