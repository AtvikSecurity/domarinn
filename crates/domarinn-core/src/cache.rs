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
                unevaluable: false,
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
                    unevaluable: false,
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
                unevaluable: false,
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
    /// Whether the request behind this verdict was replayed from cache.
    ///
    /// Reported by the grader rather than decided around it, because since
    /// 0.5.0 the grader is what consults the cache: a `similar` verdict comes
    /// from *two* embedding requests, and only the code that made them knows
    /// whether both were replays. The runner reads this straight into
    /// [`crate::result::AssertResult::cached`].
    pub cached: bool,
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
            cached: false,
        }
    }
}

/// What kind of call an entry answers.
///
/// ## Open by construction
///
/// An open string newtype rather than an enum, for
/// [`crate::error_class::ErrorClass`]'s general reason and one that is specific
/// — and decisive — here:
/// `LayeredCache` promotes a remote hit into the local tier by calling `put`
/// with the *deserialized* entry, which re-serializes it. `#[serde(other)]`
/// would collapse a kind written by a newer binary to a catch-all variant and
/// then write that back, silently rewriting a 0.7 entry's kind to garbage in
/// every local cache a 0.6 binary touched. A newtype round-trips an unknown
/// value untouched.
///
/// ## Why the writer records it rather than the reader inferring it
///
/// The persisted request cannot tell you: an `llm-rubric` judge and an OpenAI
/// provider call both serialize to
/// [`crate::provider::http_request_preview`]`("POST", url, body)`, with nothing
/// to tell them apart. Only the caller knows what it was calling, and there are
/// exactly two places that build an entry — so recording it costs two lines and
/// guessing it would cost correctness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryKind(pub String);

impl EntryKind {
    /// A response from the system under test.
    pub const PROVIDER: &'static str = "provider";
    /// An `llm-rubric` judge exchange.
    pub const JUDGE: &'static str = "judge";
    /// One half of a `similar` assertion's embedding pair.
    pub const EMBEDDING: &'static str = "embedding";
    /// An `exec` assertion's protocol round-trip.
    pub const EXEC_ASSERT: &'static str = "exec_assert";

    pub fn new(value: impl Into<String>) -> Self {
        EntryKind(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A cached provider response, plus provenance for stats/debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub created_at: DateTime<Utc>,
    /// What kind of call this entry answers. See [`EntryKind`].
    ///
    /// `None` on entries written before this field existed. Left as `None`
    /// rather than defaulted to `provider`, because a reader that wants a kind
    /// for those has to *infer* one, and inference must be able to tell "the
    /// writer said nothing" from "the writer said provider".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<EntryKind>,
    /// What *selected* the provider that answered.
    ///
    /// Optional in both directions: entries in the wild carry it, so it must
    /// keep deserializing, and it stops being written once the request itself is
    /// what the entry records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_fingerprint: Option<Json>,
    /// The redacted canonical request this entry answers. Together with the
    /// key's repeat/salt members it re-hashes to exactly this entry's key — the
    /// offline-migration and debugging contract. Stores the resolved URL /
    /// command. `None` on entries written before 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Json>,
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
    /// A digest of the local artifact that produced this response, when there
    /// was one — an `exec` provider's program, and nothing else.
    ///
    /// Evidence, not identity. It is compared against the live provider's digest
    /// on a hit so a rebuilt program is reported rather than silently replayed,
    /// and it is deliberately absent from the cache key: keying it would make
    /// every `exec` entry a property of one machine's filesystem. See
    /// [`crate::exec::program_digest`], and [`crate::cache_key`] for the same
    /// argument applied to the model a vendor reports.
    ///
    /// Absent on entries written before this field existed, and on every
    /// network-backed provider, where no such artifact exists to digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program_digest: Option<String>,
    /// Present only on a ≤0.4.x grader-verdict entry, adopted forward.
    ///
    /// It marks an era rather than a kind, and the distinction changed in
    /// 0.5.0. Through 0.4.x a grading was cached *as its verdict*, so this
    /// field's presence meant "grading result" and its absence meant "provider
    /// response". Since 0.5.0 a grader caches the request it made and re-derives
    /// the verdict from the stored `raw`, so `None` now covers both a provider
    /// response and a grader exchange, and `Some` means only one thing: an entry
    /// old enough to have no payload to re-parse. Those are served as-is and
    /// re-filed under the request key unchanged — see the `request_cache`
    /// module (private) for the read contract, and [`crate::cache_migrate`] for
    /// when this field stops being read at all.
    ///
    /// A grader lookup that finds an entry with neither a verdict nor a
    /// re-parseable payload treats it as a miss, never an error.
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
    /// The tool calls the original response reported.
    ///
    /// Replayed for the same reason as `model` and `reasoning`: a hit is the
    /// common path, so without this a `tool-call` assertion would pass on the
    /// first run of a suite and fail on every run after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<domarinn_types::result::ToolCall>,
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

/// One entry as a walk found it, before anything reads its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumeratedEntry {
    pub key: CacheKey,
    pub size: u64,
    /// Last modification time.
    ///
    /// Deliberately *not* offered as a last-access time. Access times are
    /// unreliable under `relatime`/`noatime`, and reporting mtime as one would
    /// be a different measurement wearing the same name — the mistake
    /// [`CacheEntry::provider_latency_ms`] exists to fix, in another place.
    pub modified: Option<DateTime<Utc>>,
}

/// The result of walking a store.
#[derive(Debug, Clone, Default)]
pub struct Enumerated {
    /// Newest first, so truncation drops the oldest rather than an arbitrary
    /// slice — directory order is arbitrary, and a walk cut off mid-`read_dir`
    /// would return whichever entries the filesystem happened to name first.
    pub entries: Vec<EnumeratedEntry>,
    /// True when the walk stopped at its limit with entries left unseen.
    pub truncated: bool,
    /// Files looked at, including any that were not entries.
    pub scanned: usize,
}

/// A backend that can enumerate what it holds.
///
/// Deliberately **not** a method on [`CacheBackend`]. `S3Cache` can only
/// enumerate through paginated `ListObjectsV2` — network round-trips,
/// per-request charges, and no predicate beyond a key prefix — and
/// `RemoteHttpCache` has no list verb at all. Putting `enumerate` on the core
/// trait would force both to implement something they cannot do cheaply, and a
/// backend whose enumeration is a lie is worse than one that does not offer it.
///
/// A separate trait is where "this store can be walked" gets stated, so a
/// caller that needs walking asks for it in its bound instead of hoping.
#[async_trait]
pub trait CacheEnumerate: Send + Sync {
    /// Metadata for up to `limit` entries, newest first. Cheap by contract:
    /// this stats files, it does not read them.
    async fn enumerate(&self, limit: usize) -> Result<Enumerated, CacheError>;
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

    /// Both members are optional in both directions: an entry written before
    /// `request` existed still reads, an entry written after
    /// `provider_fingerprint` stops being written still reads, and neither is
    /// serialized when absent.
    #[test]
    fn an_entry_carries_a_request_or_a_fingerprint_or_neither() {
        let legacy: CacheEntry = serde_json::from_value(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "provider_fingerprint": {"type": "exec"},
            "output": "hi",
            "domarinn_version": "0.4.0",
        }))
        .unwrap();
        assert_eq!(legacy.provider_fingerprint, Some(json!({"type": "exec"})));
        assert!(legacy.request.is_none());

        let keyed: CacheEntry = serde_json::from_value(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "hi",
            "domarinn_version": "0.5.0",
            "request": {"transport": "exec", "command": "./sut"},
        }))
        .unwrap();
        assert_eq!(keyed.request.as_ref().unwrap()["command"], json!("./sut"));
        assert!(keyed.provider_fingerprint.is_none());

        let written = serde_json::to_value(&keyed).unwrap();
        assert!(written.get("provider_fingerprint").is_none(), "{written}");
        assert_eq!(written["request"]["command"], json!("./sut"));
        assert!(serde_json::to_value(&legacy)
            .unwrap()
            .get("request")
            .is_none());
    }

    #[test]
    fn canonical_json_sorts_nested_keys() {
        let s = canonical_json(&json!({"z": {"y": 1, "x": 2}, "a": [3, 2]}));
        assert_eq!(s, r#"{"a":[3,2],"z":{"x":2,"y":1}}"#);
    }

    /// Every entry in every store predates this field. Reading one must not
    /// become an error, and absence must stay distinguishable from `provider`:
    /// the server's backfill infers a kind for `None` and adopts a `Some`
    /// verbatim, so collapsing the two would make it adopt a guess.
    #[test]
    fn an_entry_written_before_kind_existed_still_deserializes() {
        let legacy: CacheEntry = serde_json::from_value(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "provider_fingerprint": {"type": "exec"},
            "output": "hi",
            "domarinn_version": "0.4.0",
        }))
        .unwrap();
        assert!(legacy.kind.is_none());
    }

    /// The reason this is an open newtype and not an enum with
    /// `#[serde(other)]`. `LayeredCache` promotes a remote hit into the local
    /// tier by calling `put` with the *deserialized* entry, which re-serializes
    /// it — so a lossy round trip would rewrite a newer binary's kind to a
    /// catch-all in every local cache it touched.
    #[test]
    fn an_unknown_kind_survives_a_round_trip_byte_for_byte() {
        let raw = json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "hi",
            "domarinn_version": "9.9.9",
            "kind": "distillation",
        });
        let entry: CacheEntry = serde_json::from_value(raw).unwrap();
        assert_eq!(entry.kind, Some(EntryKind::new("distillation")));
        let written = serde_json::to_value(&entry).unwrap();
        assert_eq!(written["kind"], json!("distillation"));
    }

    #[test]
    fn kind_is_absent_from_the_wire_when_unset() {
        let entry: CacheEntry = serde_json::from_value(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "hi",
            "domarinn_version": "0.5.0",
        }))
        .unwrap();
        let written = serde_json::to_value(&entry).unwrap();
        assert!(written.get("kind").is_none(), "{written}");
    }

    #[test]
    fn entry_kind_constants_round_trip() {
        for name in [
            EntryKind::PROVIDER,
            EntryKind::JUDGE,
            EntryKind::EMBEDDING,
            EntryKind::EXEC_ASSERT,
        ] {
            let kind = EntryKind::new(name);
            let wire = serde_json::to_value(&kind).unwrap();
            assert_eq!(wire, json!(name));
            let back: EntryKind = serde_json::from_value(wire).unwrap();
            assert_eq!(back.as_str(), name);
        }
    }
}
