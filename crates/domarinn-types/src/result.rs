//! The versioned [`RunResult`] document — the seam between engine, CLI, and
//! server. Bump [`RESULT_SCHEMA_VERSION`] on any breaking change; golden
//! snapshots guard it.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::assert_name::AssertName;
use crate::ids::{CaseKey, RunId};
use crate::types::{Output, RenderedPrompt, TokenUsage};

/// Version of the [`RunResult`] wire document.
///
/// Version history:
/// - v3: `ChatRole::Tool` (added in 0.7.0 without a bump — retroactive
///   correction); otherwise wire-compatible with v2 — the additive
///   `RunSummary.empty_counts` is omitted at default.
/// - still v3: `CaseResult::answered_by_provider_id`,
///   `CaseResult::fallback_attempts` and `RunSummary::fallback_cases`, all
///   additive and all omitted at default, so a run that did not fall back
///   serializes byte-identically to one written before they existed. Same
///   reasoning as `empty_counts`. Note an *older* server accepts such a run and
///   then drops the three fields permanently, because it re-serializes its own
///   typed struct on ingest.
/// - v4: `CaseStatus::XFail`/`CaseStatus::XPass` (`expect_fail` cases). A new
///   *status* is not additive — a v3 reader (`FromStr`, the server's CHECK
///   constraint) rejects the strings outright — hence the bump. The
///   accompanying `CaseResult::expect_fail_reason` and
///   `RunSummary::{xfailed,xpassed}` fields *are* additive and omitted at
///   default, so a run with no `expect_fail` cases is byte-identical to v3
///   apart from the version number.
pub const RESULT_SCHEMA_VERSION: u32 = 4;

/// Identity of one cell in the provider × prompt × test × repeat matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct CellKey {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    pub test_id: String,
    #[serde(default)]
    pub repeat: u32,
}

impl CellKey {
    /// The stable cross-run identity string (16 hex chars) used for diffing and
    /// as the server's per-run primary key. Includes `repeat` so trials are
    /// distinct.
    pub fn case_key(&self) -> CaseKey {
        let mut hasher = Sha256::new();
        hasher.update(self.provider_id.as_bytes());
        hasher.update([0]);
        hasher.update(self.prompt_id.as_deref().unwrap_or("").as_bytes());
        hasher.update([0]);
        hasher.update(self.test_id.as_bytes());
        hasher.update([0]);
        hasher.update(self.repeat.to_le_bytes());
        let digest = hasher.finalize();
        let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        CaseKey::new(hex)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Pass,
    Fail,
    /// Infrastructure failure (provider error, cache-only miss). Never counted
    /// as a plain assertion failure.
    Error,
    /// Filtered out / not applicable to this cell.
    Skip,
    /// Failed, and the case was annotated `expect_fail` — the documented state
    /// of a known bug. Counted separately, like [`Self::Skip`]; never fails
    /// the gate. The explicit rename overrides `snake_case`'s `"x_fail"`.
    #[serde(rename = "xfail")]
    XFail,
    /// Passed *despite* an `expect_fail` annotation. Strict xfail: this fails
    /// the gate, because a stale annotation is a config bug — remove the
    /// marker once the underlying issue is fixed.
    #[serde(rename = "xpass")]
    XPass,
}

impl CaseStatus {
    /// The wire string for this status (identical to its serde encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            CaseStatus::Pass => "pass",
            CaseStatus::Fail => "fail",
            CaseStatus::Error => "error",
            CaseStatus::Skip => "skip",
            CaseStatus::XFail => "xfail",
            CaseStatus::XPass => "xpass",
        }
    }
}

impl std::str::FromStr for CaseStatus {
    type Err = String;

    /// Strict parse of the same strings [`CaseStatus::as_str`] produces.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pass" => Ok(CaseStatus::Pass),
            "fail" => Ok(CaseStatus::Fail),
            "error" => Ok(CaseStatus::Error),
            "skip" => Ok(CaseStatus::Skip),
            "xfail" => Ok(CaseStatus::XFail),
            "xpass" => Ok(CaseStatus::XPass),
            other => Err(format!(
                "invalid case status '{other}'; expected one of: pass, fail, error, skip, xfail, xpass"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AssertStatus {
    Pass,
    Fail,
    Error,
    /// Not evaluated because the case was already decided (short-circuit).
    Skipped,
}

impl AssertStatus {
    /// The status of a graded assertion from its boolean outcome.
    ///
    /// A constructor for the wire value, so it lives with the type rather than
    /// with whichever engine module happens to call it.
    pub fn from_pass(pass: bool) -> AssertStatus {
        if pass {
            AssertStatus::Pass
        } else {
            AssertStatus::Fail
        }
    }
}

/// The result of a single assertion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct AssertResult {
    pub kind: AssertName,
    pub status: AssertStatus,
    pub score: f64,
    pub weight: f64,
    #[serde(default)]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// The assertion's definition — its type-specific criteria as authored (the
    /// `contains` substring, the `llm-rubric` rubric text + threshold, …), plus
    /// a `negate: true` entry when the assertion is negated. `weight` is omitted
    /// (already a field above). Absent on pre-v2.1 stored blobs; presence-gated
    /// by the web UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<serde_json::Value>,
    #[serde(default)]
    pub cached: bool,
    /// What producing this verdict cost, for the assertions that call a model
    /// to decide (`llm-rubric`, `similar`). Absent for a locally-evaluated
    /// assertion, which costs nothing, and for an `exec` grader, whose spending
    /// domarinn cannot see.
    ///
    /// Deliberately *not* folded into `CaseResult.cost_usd`: that number is what
    /// the system under test cost, which is what a `cost:` assertion budgets. A
    /// judge's price should not move a budget gate on the model being judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// What kind of failure this assertion's error was — set exactly when
    /// `status` is `Error`; absent otherwise and on pre-existing blobs. This
    /// is the *uncollapsed* record: the case-level `error_class` keeps one
    /// class per case (harness tier winning, so the exit code is right), and
    /// a mixed case's dropped suite class used to be recoverable from nowhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<crate::error_class::ErrorClass>,
}

/// One tool call a model decided to make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct ToolCall {
    /// The vendor's call id, when there was one. Carried so a multi-call
    /// response stays attributable; never interpreted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    /// The decoded arguments object — never the raw JSON string some vendors
    /// put on the wire, which would hand every assertion a parsing problem
    /// instead of an argument.
    #[serde(default)]
    #[ts(type = "unknown")]
    pub arguments: serde_json::Value,
}

/// The result of one matrix cell.
///
/// The web UI receives this document verbatim from `GET /runs/{id}/cases/{key}`
/// (the stored blob, not a re-derived DTO), so every `skip_serializing_if`
/// here means "key absent on the wire". Field-level `#[ts(optional)]` is used
/// instead of struct-level `#[ts(optional_fields)]` because the struct-level
/// form only marks `Option` fields and would emit `tags` as required even
/// though it is skipped when empty; without it, ts-rs's serde-aware fallback
/// (`default` + `skip_serializing_if`) marks `tags` optional too.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct CaseResult {
    pub cell: CellKey,
    pub case_key: CaseKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The rendered test variables that produced this cell (the substituted
    /// `vars` map, environment excluded — the same values fed to the provider
    /// request). Empty when the test had no vars, and absent on pre-v2.1 stored
    /// blobs; presence-gated by the web UI. Marked optional in TS via ts-rs's
    /// serde-aware fallback (`default` + `skip_serializing_if`), same as `tags`.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub vars: serde_json::Map<String, serde_json::Value>,
    pub status: CaseStatus,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output: Option<Output>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prompt: Option<RenderedPrompt>,
    /// The request the provider actually sent — the payload the model received,
    /// captured from the same code path that performs the call
    /// ([`crate::provider::Provider::request_preview`]).
    ///
    /// `prompt` above is what domarinn rendered; this is what crossed the wire,
    /// including the model id and every sampling parameter. The two differ in
    /// ways that matter when debugging: a `max_tokens` visible here next to a
    /// `length` stop reason explains a truncated case immediately.
    ///
    /// Absent when the provider declines to describe its request (the HTTP
    /// provider does, to avoid persisting `env`-templated credentials) and on
    /// stored blobs written before this field existed; presence-gated by the web
    /// UI. Never contains headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub request: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stop_reason: Option<String>,
    /// The model the provider actually used, as reported by the provider —
    /// not the one the suite configured.
    ///
    /// The two diverge whenever a suite pins a floating alias and the vendor
    /// repoints it, which otherwise has no signal at all: the configured model
    /// is in the provider fingerprint, so the cache and every comparison agree
    /// nothing changed while the thing being measured did. `None` for providers
    /// with no model concept, and for runs recorded before this existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    /// The tool calls the model decided to make, in order.
    ///
    /// A *decision*, not an execution: domarinn never runs a tool. Recording
    /// them is what makes a case whose right answer is a tool call gradeable at
    /// all — previously such a case had no prose, scored zero, and looked like
    /// a model failure rather than an evaluation that could not see the answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub raw: Option<serde_json::Value>,
    #[serde(default)]
    pub asserts: Vec<AssertResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cost_usd: Option<f64>,
    /// Time the provider itself took, excluding retry backoff and (on a cache
    /// hit) replayed from the entry rather than measured against the cache read.
    ///
    /// This is what `latency` assertions grade. Keeping backoff out of it is
    /// not cosmetic: with retries enabled, a single 429 would otherwise fail
    /// `{type: latency, max: 2000}` on a model that answered in 30 ms.
    pub latency_ms: u64,
    /// Total wall time for the cell, *including* retry backoff and cache I/O.
    /// Absent on blobs written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wall_ms: Option<u64>,
    #[serde(default)]
    pub cached: bool,
    /// The cache key this case's provider call was addressed by, when it had
    /// one.
    ///
    /// Recorded whether the call hit or missed. The key is a property of the
    /// *request*, so a miss that wrote an entry and a hit that read one carry
    /// the same value — which is what makes "which runs used this entry"
    /// answerable at all, rather than only "which runs were cache hits".
    ///
    /// `None` is a real and common state, not just a legacy one: `--no-cache`,
    /// a provider that declines caching, a call with no stable canonical
    /// request, and every run recorded before this field existed.
    ///
    /// A `String`, not a `CacheKey`: that type lives in `domarinn-core`, which
    /// `tests/boundary.rs` forbids this crate from depending on.
    ///
    /// Grader keys are deliberately absent. One case makes up to three graded
    /// calls, each with its own key; if those are ever wanted they belong on
    /// [`AssertResult`], not folded into one field here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cache_key: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    /// Identity of what this case asked the model — rendered prompt, rendered
    /// vars, params, cache salt. Deliberately excludes the provider (that is
    /// `provider_digest`) and `repeat`. See [`crate::digests::prompt_digest`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prompt_digest: Option<String>,
    /// Identity of the model and its settings, from the provider fingerprint.
    ///
    /// **Not backfillable**: the fingerprint appears nowhere in a stored run
    /// document (`request` is a preview envelope and is absent under
    /// `--no-raw`), so change classification only works between two runs that
    /// both post-date this field. Readers must treat `None` as unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider_digest: Option<String>,
    /// Identity of this case's grading definition — the authored criteria,
    /// weights and threshold, never the outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub assert_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    /// Structured detail for whatever `error` describes — the machine-readable
    /// half of a message that is otherwise prose.
    ///
    /// Provider-authored for a provider failure. It exists because `error` was
    /// the only field that survived a failed call, so anything structured had
    /// to be formatted into a sentence and parsed back out by whoever read it.
    ///
    /// Size-capped like `raw`, but deliberately **not** gated behind
    /// `--no-raw`: that flag exists to shrink documents by dropping bulky
    /// happy-path provenance, and an errored case has no `output` and no `raw`,
    /// so this is the only thing it carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "unknown")]
    pub error_details: Option<serde_json::Value>,
    /// What kind of failure `error` describes — see [`crate::error_class`].
    ///
    /// Strictly additive: `error` stays the verbatim prose it always was, and
    /// neither field is ever derived from the other at read time.
    /// `error_class.is_some()` implies `error.is_some()`; the converse does not
    /// hold, because blobs written before this field have prose and no class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error_class: Option<crate::error_class::ErrorClass>,
    /// The model's reasoning/thinking text, when it exposed any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reasoning: Option<String>,
    /// Why `output` has nothing gradeable in it. Set whenever the output is
    /// empty, including when reasoning was substituted for it — renderers must
    /// key on this being present, never on the output looking empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub empty_reason: Option<crate::empty::EmptyReason>,
    /// The provider that actually answered, when it was not the one
    /// [`Self::cell`]`.provider_id` names — that is, when a `fallback:` chain
    /// was walked.
    ///
    /// `cell.provider_id` stays the **configured** provider so `case_key` is
    /// stable and an `--against` baseline still joins the same row. This is
    /// where the truth is recorded. `provider_digest` reflects the same
    /// provider, so a fallback classifies as `ProviderChanged` in a diff —
    /// honest, because a different model answered.
    ///
    /// The server promotes this onto its `cases` table and attributes
    /// per-provider cost rollups to the answering provider; only rows ingested
    /// before that column existed degrade to configured-provider attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub answered_by_provider_id: Option<String>,
    /// The chain links tried and passed over before the one that answered, in
    /// configured order.
    ///
    /// Absent — not an empty array — for the overwhelming majority of cases, so
    /// a run that never fell back serializes byte-identically to one from
    /// before this field existed. See the note on
    /// [`RunSummary::cache_read_tokens`] for why that matters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_attempts: Vec<FallbackAttempt>,
    /// The reason string from the case's `expect_fail` annotation, when it
    /// carried one. Present whenever the annotation had a reason — including
    /// on an [`CaseStatus::Error`] or [`CaseStatus::Skip`] case, because the
    /// annotation is a fact about the case, not about this run's outcome.
    /// Documentation only: it never participates in grading or digests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expect_fail_reason: Option<String>,
}

impl CaseResult {
    /// The provider whose answer this case reports: the fallback link when one
    /// answered, otherwise the configured provider the cell names.
    ///
    /// The one place the `answered_by ?? configured` collapse is defined.
    /// Every surface that shows "who answered" (diff annotations, tables,
    /// rollups) goes through here rather than re-deriving it, so the rule
    /// cannot fork per consumer.
    pub fn answering_provider_id(&self) -> &str {
        self.answered_by_provider_id
            .as_deref()
            .unwrap_or(&self.cell.provider_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct RunSummary {
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
    pub errored: u64,
    pub skipped: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub cache_hits: u64,
    #[serde(default)]
    pub cache_misses: u64,
    /// Cases that needed more than one attempt. Distinct from `errored`: a
    /// retried case usually *succeeded*, and without this the only trace of the
    /// cost is a longer wall-clock.
    #[serde(default)]
    pub retried_cases: u64,
    /// Input tokens served from a provider-side prompt cache, summed.
    ///
    /// Note these three carry `skip_serializing_if`, unlike the bare
    /// `#[serde(default)]` counters above. That is deliberate and not a style
    /// inconsistency: those fields predate the server's content-hash ingest, so
    /// they already appear in every stored run. Adding a *new* always-emitted
    /// counter would make every historical run grow a `0` on re-serialization
    /// and shift the hash the server uses for idempotency, turning a re-upload
    /// into a 409. Guarded by
    /// `a_run_without_optional_provenance_does_not_grow_it_on_re_serialization`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read_tokens: u64,
    /// Input tokens written into a provider-side prompt cache, summed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write_tokens: u64,
    /// What the cached cases would have cost had they been called.
    ///
    /// Exact arithmetic on data already present — the sum of `cost_usd` over
    /// cases that were cache hits — not a counterfactual re-pricing. Note
    /// `cost_usd` above is the cost of the *work*, so money actually spent this
    /// run is `cost_usd - cache_savings_usd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_savings_usd: Option<f64>,
    /// What the graders cost, summed over every assertion that called a model
    /// to reach its verdict.
    ///
    /// Separate from `cost_usd` rather than added to it, because the two answer
    /// different questions: `cost_usd` is what the systems under test cost — the
    /// number a `cost:` assertion budgets and a model-selection decision turns
    /// on — while this is what measuring them cost. On a suite graded by a
    /// larger model than it tests, this is the bigger of the two, and adding
    /// them would hide that rather than report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grader_cost_usd: Option<f64>,
    /// Cases whose output carried an `empty_reason`, tallied by reason string.
    ///
    /// Open-keyed because the reason set is open (see [`crate::empty`]), and a
    /// `BTreeMap` so serialization is deterministic — the stored document is
    /// content-hashed. Absent when empty, per the byte-stability rule above.
    ///
    /// The `ts` attribute makes the generated TS say `empty_counts?:` rather
    /// than a mandatory field the JSON then omits. It has to go through `as`:
    /// bare `#[ts(optional)]` is a compile error on a non-`Option` field
    /// (ts-rs implements `IsOption` only for `Option<T>`), and the struct-level
    /// `optional_fields` above marks only `Option` fields. Unlike the numeric
    /// counters — where a consumer writing `?? 0` gets the right answer — an
    /// undefined map reaching `Object.entries()` throws.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    #[ts(as = "Option<std::collections::BTreeMap<String, u64>>", optional)]
    pub empty_counts: std::collections::BTreeMap<String, u64>,
    /// *Graded* cases answered by a provider's `fallback:` chain rather than by
    /// the provider the cell names. A skipped case does not count, whoever
    /// answered it: it graded nothing.
    ///
    /// A run where this equals the graded count (`total - skipped`) is refused
    /// by the CLI: every graded case came from somewhere other than the system
    /// under test, so a green gate would mean the suite ran, not that it
    /// passed. A partial fallback is still green — that is the feature working.
    ///
    /// Absent at zero, for the byte-stability reason documented on
    /// `cache_read_tokens` above.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fallback_cases: u64,
    /// Cases that failed while annotated `expect_fail` ([`CaseStatus::XFail`]).
    /// Not folded into `failed` — an expected failure never fails the gate.
    /// Absent at zero, per the byte-stability rule on `cache_read_tokens`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub xfailed: u64,
    /// Cases that passed while annotated `expect_fail` ([`CaseStatus::XPass`]).
    /// Fails the gate exactly like `failed` (strict xfail), but counted apart
    /// so the report can say *why*. Absent at zero, same rule.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub xpassed: u64,
}

/// One `fallback:` link that was tried and passed over, and why.
///
/// Exactly one of [`Self::empty_reason`] / [`Self::error_class`] is set: a link
/// either answered with something the suite declined to accept, or failed to
/// answer at all. Both are open string newtypes, so a reason or class this
/// build has never heard of round-trips rather than being dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
// No `export`: every other type in this crate is reached transitively by the
// `gen-types` binary, and `#[ts(export)]` would make `cargo test` write a
// stray, untracked `crates/domarinn-types/bindings/` directory as a side effect.
#[ts(optional_fields)]
pub struct FallbackAttempt {
    /// The provider that was tried and did not answer.
    pub provider_id: String,
    /// Set when the link replied, but with nothing gradeable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub empty_reason: Option<crate::empty::EmptyReason>,
    /// Set when the link failed to reply at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error_class: Option<crate::error_class::ErrorClass>,
}

/// `skip_serializing_if` helper for counters that must stay absent at zero.
fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct GitMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default)]
    pub dirty: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct CiMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
}

/// Who and what produced a run.
///
/// Collected by the engine (see [`crate::provenance`]) so a plain `domarinn run`
/// carries it too. Before this existed, git and CI metadata were attached only
/// on the upload path, so every `result.json` on disk had `git: null` and there
/// was no notion of an actor at all.
///
/// [`RunResult::git`] and [`RunResult::ci`] deliberately stay as their own
/// fields rather than moving in here: they already have server columns and a
/// share-time backfill path, so folding them in would be a breaking rename for
/// no gain.
///
/// One optional struct rather than five optional fields, also deliberately. The
/// byte-stable-reserialization contract is about keys staying *absent* on
/// documents that never had them; nesting costs exactly one
/// `skip_serializing_if` at the [`RunResult`] level and leaves every field
/// inside it unconstrained for legacy blobs, which carry no `origin` key at all.
/// It also gives the privacy opt-out a single lever: `origin: None`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct RunOrigin {
    /// Who this run is attributable to: the CI actor when running in CI
    /// (`GITHUB_ACTOR` and friends — of a CI run people ask "who pushed the
    /// change", not "which service account ran the container"), otherwise the OS
    /// username. `None` when suppressed or undeterminable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Hostname of the machine that ran it. `None` when suppressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The domarinn build that produced this document ([`crate::VERSION`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A free-form human label (`domarinn run --note "…"`), falling back to the
    /// suite's `description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Set when the identity fields were suppressed by policy. Distinguishes "the
    /// operator opted out" from "an older client wrote this document" — without
    /// it a reader cannot tell the two apart, since both look like absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
}

/// Component digests of the suite that produced a run — see [`crate::digests`].
///
/// [`RunResult::config_digest`] is one hash over the whole suite: it answers
/// "did anything change" and nothing else. These answer *what*, so a comparison
/// can say "the prompts changed" instead of "config changed", and the runs list
/// can say it from the row without fetching a config snapshot per run.
///
/// All optional: a run written before this existed has none, and `None` must
/// read as "unknown", never as "unchanged".
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct ConfigDigests {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asserts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grader: Option<String>,
}

/// Provenance of a synthetic branch-baseline document.
///
/// A branch baseline is a *merge*: per [`CaseResult::case_key`], the newest run
/// on the branch that has the case wins, so a filtered or sharded newest run
/// cannot silently shrink the gate's coverage. The merged document is built on
/// demand for comparison and never persisted or uploaded — this field exists so
/// a renderer can say which real runs the synthetic baseline was drawn from.
///
/// Additive and omitted when absent: every stored document predates it and must
/// re-serialize byte-identically (the server's content-hash idempotency), so no
/// [`RESULT_SCHEMA_VERSION`] bump.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct CompositeBaseline {
    /// The git branch the contributing runs were selected from.
    pub branch: String,
    /// The real runs that contributed at least one case, newest first.
    pub contributing_run_ids: Vec<RunId>,
    /// Set when the lookback cap was reached while runs were still contributing
    /// new cases — older coverage may exist beyond the horizon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// Which filters produced this run (for reproducibility and the UI).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
pub struct FilterSpec {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
}

/// The full result of a run.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct RunResult {
    pub schema_version: u32,
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub config_digest: String,
    pub config_snapshot: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiMeta>,
    /// Who and where this run came from. See [`RunOrigin`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<RunOrigin>,
    /// Per-component digests of the suite. See [`ConfigDigests`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digests: Option<ConfigDigests>,
    /// Where this run was published, recorded by `--share` from the URL the
    /// results server returned. Absent when the run was never shared.
    ///
    /// Stored rather than re-derived so a later reader — `ci-summary` building a
    /// PR comment, most of all — can link to the run without re-uploading it or
    /// parsing it back out of an earlier command's stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
    /// Set only on a synthetic branch-baseline document assembled by merging
    /// runs — see [`CompositeBaseline`]. Always absent on a real run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composite: Option<CompositeBaseline>,
    #[serde(default)]
    pub filters: FilterSpec,
    pub cases: Vec<CaseResult>,
    pub summary: RunSummary,
}

#[cfg(test)]
#[path = "result_tests.rs"]
mod result_tests;
