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
    /// The URL this response actually came from, when the provider has a fixed
    /// one.
    ///
    /// Evidence, not identity — the exact role [`Self::program_digest`] plays for
    /// an `exec` provider's bytes. `base_url` is deliberately absent from the
    /// cache key so that a gateway and a direct connection to the same API share
    /// entries instead of paying twice; this is what keeps that from being
    /// silent. On a hit it is compared against where the request is *now*
    /// addressed, and a mismatch is reported once per provider.
    ///
    /// Absent on entries written before this field existed, and on `exec` and
    /// `http` providers — the first has no URL, the second puts its templated
    /// one in the canonical request already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
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

impl CacheEntry {
    /// A `similar` assertion's ≤0.4.x cosine verdict.
    ///
    /// Not [`EntryKind::EMBEDDING`]: an embedding entry holds one vector, where
    /// this holds the comparison of two. Different things that happen to come
    /// from the same assertion, and collapsing them would make a kind filter
    /// return entries that cannot answer the same question.
    pub const SIMILAR_VERDICT: &'static str = "similar_verdict";

    /// What kind of call this entry answers, first match wins.
    ///
    /// The rungs are ordered by how much they are *claiming*. An explicit kind is a
    /// statement the writer made; everything below it is a reader's guess, and a
    /// guess must never overrule a statement — including a kind string this build
    /// has never heard of, which is exactly the case an open
    /// [`EntryKind`] exists to carry.
    pub fn inferred_kind(&self) -> Option<String> {
        // 1. The writer said so.
        if let Some(kind) = &self.kind {
            return Some(kind.as_str().to_string());
        }

        // 2. A ≤0.4.x verdict entry self-identifies: `GradedVerdict` is
        //    `#[serde(tag = "kind")]`, so the stored shape names its own grader.
        if let Some(verdict) = &self.verdict {
            return Some(
                match verdict {
                    GradedVerdict::Rubric { .. } => EntryKind::JUDGE,
                    GradedVerdict::Exec { .. } => EntryKind::EXEC_ASSERT,
                    GradedVerdict::Similarity { .. } => CacheEntry::SIMILAR_VERDICT,
                }
                .to_string(),
            );
        }

        // 3. An `exec` request carries the protocol envelope it wrote to the
        //    child's stdin, and that envelope names the call kind.
        if let Some(request) = &self.request {
            if request.get("transport").and_then(Json::as_str) == Some("exec") {
                match request
                    .pointer("/stdin/domarinn/kind")
                    .and_then(Json::as_str)
                {
                    Some("provider") => return Some(EntryKind::PROVIDER.to_string()),
                    Some("assert") => return Some(EntryKind::EXEC_ASSERT.to_string()),
                    _ => {}
                }
            }
        }

        // 4. An embedding's output is a dimension count and nothing else — the
        //    grader writes that shape deliberately so an entry is legible.
        if let Output::Json(value) = &self.output {
            if let Some(map) = value.as_object() {
                if map.len() == 1 && map.contains_key("dims") {
                    return Some(EntryKind::EMBEDDING.to_string());
                }
            }
        }

        // 5. Only the provider path goes through `with_retry`, so only it has an
        //    attempt count or an in-flight measurement to record. The grader path
        //    sets both to `None` and says why.
        if self.attempts.is_some() || self.provider_latency_ms.is_some() {
            return Some(EntryKind::PROVIDER.to_string());
        }

        // 6. Nothing distinguishes an old HTTP judge call from an old HTTP provider
        //    call. No kind is the honest answer; a default would be a fabrication
        //    that a filter would then treat as fact.
        None
    }
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

/// Which entries a prune should remove.
///
/// Two families of predicate, and the split is load-bearing rather than
/// cosmetic. *Age* predicates are answerable from a `stat`; *body* predicates
/// require reading and parsing the entry. A store with 200k entries can be
/// swept by age for the cost of a directory walk, and asking it to parse every
/// file to answer a question nobody posed would turn the retention path into a
/// minutes-long job. [`Self::is_age_only`] is what lets a backend keep the
/// cheap walk cheap.
///
/// The other structural rule is [`Self::is_default`]. A prune whose filter
/// names nothing must mean "everything" — that is `cache clear` — but that
/// reading must be reachable *only* from a literally-empty filter. Deriving it
/// instead from "no predicate matched" is how `--newer-than 1h` alone comes to
/// delete the whole store, and how an empty `empty_reason` vec comes to make
/// `clear` a no-op. Backends are expected to write
/// `filter.is_default() || (age && body)` rather than fold the two together.
#[derive(Debug, Clone, Default)]
pub struct PurgeFilter {
    /// Remove entries last modified *before* now minus this.
    pub older_than: Option<chrono::Duration>,
    /// Remove entries last modified *after* now minus this.
    ///
    /// Only ever meaningful as the other half of a window: on its own it names
    /// "everything since", which is a whole-store wipe wearing the vocabulary
    /// of housekeeping. The CLI refuses to construct that; this type merely
    /// carries it.
    pub newer_than: Option<chrono::Duration>,
    /// Remove entries whose stored [`crate::empty::EmptyReason`] is any of
    /// these. Empty means "do not filter on the reason at all" — *not* "match
    /// entries with no reason".
    pub empty_reason: Vec<crate::empty::EmptyReason>,
    /// Remove entries answered by exactly this model.
    pub model: Option<String>,
    /// Remove entries of exactly this kind, compared against
    /// [`CacheEntry::inferred_kind`].
    pub kind: Option<String>,
}

impl PurgeFilter {
    /// True when no predicate is set at all — the `cache clear` filter.
    ///
    /// The single gate for "remove everything". Every other path in a backend
    /// must go through the per-predicate arms, so that adding a predicate can
    /// only ever *narrow* what is deleted. A filter that names something and
    /// matches nothing removes nothing; only this returning `true` removes the
    /// store.
    pub fn is_default(&self) -> bool {
        self.older_than.is_none()
            && self.newer_than.is_none()
            && self.empty_reason.is_empty()
            && self.model.is_none()
            && self.kind.is_none()
    }

    /// True when nothing here needs the entry's body — only its timestamp.
    ///
    /// The disk backend's fast path. `cache gc --older-than 30d` is the
    /// retention sweep people run on a cron against a store that may hold
    /// hundreds of thousands of files; answering it from `stat` alone is the
    /// difference between a directory walk and deserializing every entry on
    /// disk. Note this is *not* the negation of [`Self::is_default`]: a
    /// literally-default filter is also age-only, which is exactly right —
    /// `clear` reads no bodies either.
    pub fn is_age_only(&self) -> bool {
        self.empty_reason.is_empty() && self.model.is_none() && self.kind.is_none()
    }

    /// Whether an entry's *body* satisfies this filter. Timestamps are the
    /// backend's business, since only it knows where they come from.
    ///
    /// Every unset predicate is vacuously true, and they are ANDed: naming a
    /// model and a kind means "entries that are both", never "either". The
    /// alternative — ORing — would make each added predicate widen the blast
    /// radius of a destructive command, which is the wrong direction for a
    /// filter whose only job is to spare things.
    pub fn matches_entry(&self, entry: &CacheEntry) -> bool {
        // Compared by string, not by parsing into a known set. `EmptyReason` is
        // an open newtype on purpose (vendors add finish reasons without a
        // domarinn release), and this must not become the place that quietly
        // closes it — an operator evicting a reason this binary has never heard
        // of is precisely the recovery case the filter exists for.
        let reason_ok = self.empty_reason.is_empty()
            || entry.empty_reason.as_ref().is_some_and(|have| {
                // Compared in the clamped form both tiers store, so the same
                // `--empty-reason` value selects the same entries whether it is
                // answered by a local disk walk or by the server's promoted
                // column. See `EmptyReason::clamped`.
                self.empty_reason
                    .iter()
                    .any(|want| want.clamped() == have.clamped())
            });

        let model_ok = match &self.model {
            None => true,
            Some(want) => entry.model.as_deref() == Some(want.as_str()),
        };

        // Against the *inferred* kind rather than the stored one, so that the
        // disk tier and the server's browse tier agree on what `--kind judge`
        // selects. An entry written before `kind` existed still has a kind a
        // reader can derive, and a filter that ignored it would report "0
        // removed" for entries plainly listed as judges.
        let kind_ok = match &self.kind {
            None => true,
            Some(want) => entry.inferred_kind().as_deref() == Some(want.as_str()),
        };

        reason_ok && model_ok && kind_ok
    }
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

    fn entry(json: Json) -> CacheEntry {
        serde_json::from_value(json).expect("test fixture must deserialize")
    }

    fn plain_entry() -> CacheEntry {
        entry(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "hi",
            "domarinn_version": "0.9.0",
        }))
    }

    /// `cache clear` is a default filter, and it must keep removing everything.
    /// The hazard is a backend deriving "match all" from "no predicate rejected
    /// it": add one predicate whose unset form is a `contains()` on an empty
    /// `Vec` and `clear` silently becomes a no-op.
    #[test]
    fn a_default_purge_filter_matches_everything() {
        let filter = PurgeFilter::default();
        assert!(filter.is_default());
        assert!(filter.is_age_only(), "clear reads no bodies either");
        assert!(filter.matches_entry(&plain_entry()));

        let poisoned = entry(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "",
            "empty_reason": "refusal",
            "model": "claude-x",
            "kind": "provider",
            "domarinn_version": "0.9.0",
        }));
        assert!(filter.matches_entry(&poisoned));
    }

    /// An entry with no reason recorded is not "a reason we did not name" — it
    /// is an entry the predicate cannot speak about, so it survives. Otherwise
    /// `gc --empty-reason refusal` would take every ordinary answer with it.
    #[test]
    fn an_empty_reason_predicate_never_matches_an_entry_without_one() {
        let filter = PurgeFilter {
            empty_reason: vec![crate::empty::EmptyReason::new(
                crate::empty::EmptyReason::REFUSAL,
            )],
            ..Default::default()
        };
        assert!(!filter.is_default());
        assert!(!filter.is_age_only(), "a reason predicate needs the body");
        assert!(!filter.matches_entry(&plain_entry()));

        let refused = entry(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "",
            "empty_reason": "refusal",
            "domarinn_version": "0.9.0",
        }));
        assert!(filter.matches_entry(&refused));

        // Open by construction: a reason this binary has never heard of is
        // still evictable, which is the whole point of naming one.
        let future = PurgeFilter {
            empty_reason: vec![crate::empty::EmptyReason::new("invented_next_year")],
            ..Default::default()
        };
        let odd = entry(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "",
            "empty_reason": "invented_next_year",
            "domarinn_version": "9.9.9",
        }));
        assert!(future.matches_entry(&odd));
        assert!(!future.matches_entry(&refused));
    }

    /// Naming two predicates must *narrow* a destructive command, never widen
    /// it. ORing would make `--model x --kind judge` delete every judge entry
    /// plus every entry from `x`, which is not what either flag says.
    #[test]
    fn body_predicates_and_rather_than_or() {
        let filter = PurgeFilter {
            model: Some("claude-x".into()),
            kind: Some(EntryKind::JUDGE.into()),
            ..Default::default()
        };

        let both = entry(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "hi",
            "model": "claude-x",
            "kind": "judge",
            "domarinn_version": "0.9.0",
        }));
        assert!(filter.matches_entry(&both));

        let model_only = entry(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "hi",
            "model": "claude-x",
            "kind": "provider",
            "domarinn_version": "0.9.0",
        }));
        assert!(!filter.matches_entry(&model_only));

        let kind_only = entry(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "hi",
            "model": "other",
            "kind": "judge",
            "domarinn_version": "0.9.0",
        }));
        assert!(!filter.matches_entry(&kind_only));
    }

    /// `kind` reads [`CacheEntry::inferred_kind`], so an entry written before
    /// the field existed is still selectable by the kind every reader — the
    /// disk tier and the server's listing alike — already shows for it.
    #[test]
    fn a_kind_predicate_matches_an_inferred_kind() {
        let filter = PurgeFilter {
            kind: Some(EntryKind::PROVIDER.into()),
            ..Default::default()
        };
        let legacy = entry(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "hi",
            "attempts": 1,
            "domarinn_version": "0.4.0",
        }));
        assert!(legacy.kind.is_none(), "no kind was written");
        assert!(filter.matches_entry(&legacy));
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
