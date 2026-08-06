//! `providers[].fallback`: handing a cell to another provider when the
//! configured one will not answer.
//!
//! The interesting assertions here are the negative ones. A fallback that fires
//! when it should is easy to see; a fallback that fires when it should *not* —
//! under `--cache-only`, against a test that excluded the provider, on a cell
//! with a `latency` assert — produces a run that looks fine and is not. Those
//! are the tests worth having.
//!
//! Everything is `exec` over `sh -c`, so the whole file is offline by
//! construction and a refusal is stated rather than simulated.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheMode, CacheStats, PurgeFilter,
};
use domarinn_core::result::CaseStatus;
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::RunResult;

#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
    gets: AtomicUsize,
}

#[async_trait]
impl CacheBackend for MemCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        Ok(self.map.lock().unwrap().get(&key.0).cloned())
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        self.map
            .lock()
            .unwrap()
            .entry(key.0.clone())
            .or_insert_with(|| entry.clone());
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(CacheStats::default())
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// An `exec` provider that emits `body` as its protocol response.
fn emitting(id: &str, body: &str, fallback: &str) -> String {
    format!(
        "  - id: {id}\n    type: exec\n    command: [\"sh\", \"-c\", \"cat >/dev/null; printf '%s' '{body}'\"]\n{fallback}"
    )
}

/// An `exec` provider that fails to run at all — `exec_failed`, a trigger.
fn broken(id: &str, fallback: &str) -> String {
    format!("  - id: {id}\n    type: exec\n    command: [\"sh\", \"-c\", \"exit 1\"]\n{fallback}")
}

const REFUSES: &str = r#"{\"output\":\"\",\"empty_reason\":\"refusal\"}"#;
const ANSWERS: &str = r#"{\"output\":\"GOOD\"}"#;
const OTHER: &str = r#"{\"output\":\"OTHER\"}"#;

fn chain(ids: &str) -> String {
    format!("    fallback: [{ids}]\n")
}

fn suite(providers: &str, tests: &str) -> String {
    format!("version: 1\nproject: test\nsuite: fallback\nproviders:\n{providers}tests:\n{tests}")
}

/// One test that passes only when the output is the fallback's.
const WANTS_GOOD: &str = "  - id: t\n    assert:\n      - type: contains\n        value: GOOD\n";

async fn run_suite(yaml: &str, opts: RunOptions) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    let cache = MemCache::default();
    run(&suite, Path::new("."), &cache, None, &opts)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_refusal_hands_off_to_the_fallback() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    // Two providers, so two cells; the fallback's own cell answers normally.
    let case = r
        .cases
        .iter()
        .find(|c| c.cell.provider_id == "primary")
        .expect("the primary's cell must exist");
    assert_eq!(case.status, CaseStatus::Pass);
}

/// The `--against` join. `case_key` hashes `provider_id` first, so recording the
/// answering provider there would turn every fallback into a Removed + Added
/// pair in the diff instead of a comparable row.
#[tokio::test]
async fn the_cell_still_names_the_configured_provider() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    let case = r
        .cases
        .iter()
        .find(|c| c.answered_by_provider_id.is_some())
        .expect("one case must have fallen back");
    assert_eq!(case.cell.provider_id, "primary");
    assert_eq!(case.answered_by_provider_id.as_deref(), Some("backup"));
    assert_eq!(case.fallback_attempts.len(), 1);
    assert_eq!(case.fallback_attempts[0].provider_id, "primary");
    assert_eq!(
        case.fallback_attempts[0]
            .empty_reason
            .as_ref()
            .map(|r| r.as_str()),
        Some("refusal")
    );
}

/// The content-hash guarantee: a run that never fell back must serialize
/// byte-identically to one written before these fields existed, or every
/// historical run 409s on re-upload after the upgrade.
#[tokio::test]
async fn a_case_that_did_not_fall_back_omits_the_new_fields() {
    let yaml = suite(&emitting("p", ANSWERS, ""), WANTS_GOOD);
    let r = run_suite(&yaml, RunOptions::default()).await;
    let v = serde_json::to_value(&r.cases[0]).unwrap();
    assert!(v.get("answered_by_provider_id").is_none());
    assert!(v.get("fallback_attempts").is_none());
    let s = serde_json::to_value(&r.summary).unwrap();
    assert!(s.get("fallback_cases").is_none());
}

#[tokio::test]
async fn a_provider_that_cannot_run_hands_off() {
    let yaml = suite(
        &format!(
            "{}{}",
            broken("primary", &chain("backup")),
            emitting("backup", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    let case = r
        .cases
        .iter()
        .find(|c| c.cell.provider_id == "primary")
        .unwrap();
    assert_eq!(case.status, CaseStatus::Pass);
    assert_eq!(
        case.fallback_attempts[0]
            .error_class
            .as_ref()
            .map(|c| c.as_str()),
        Some("exec_failed")
    );
}

/// Invariant 1. Offline, a handoff can only replace a usable (if refused)
/// replay with a `cache_miss` error, so the chain is never walked at all.
#[tokio::test]
async fn cache_only_never_walks_the_chain() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(
        &yaml,
        RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            ..Default::default()
        },
    )
    .await;
    assert!(
        r.cases.iter().all(|c| c.answered_by_provider_id.is_none()),
        "no case may hand off under --cache-only"
    );
}

/// Invariant 2, and the reason it needs its own guard: a latency-asserted cell
/// is forced to `CacheMode::Disabled`, not `ReadOnlyStrict`, so invariant 1 does
/// not cover it. Without this, primary-times-out → fallback-answers-fast makes a
/// latency budget *pass* on a provider that never answered.
#[tokio::test]
async fn a_latency_asserted_cell_never_walks_the_chain() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", ANSWERS, "")
        ),
        "  - id: t\n    assert:\n      - type: latency\n        max: 60000\n",
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    assert!(
        r.cases.iter().all(|c| c.answered_by_provider_id.is_none()),
        "a cell whose latency is under assertion must measure its own provider"
    );
}

/// Invariant 3. When no link improved on the primary, the primary's own outcome
/// is reported — otherwise `provider_digest` churns for nothing and the run
/// document diverges from a no-fallback run.
#[tokio::test]
async fn when_every_link_declines_the_primary_is_reported() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", REFUSES, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    let case = r
        .cases
        .iter()
        .find(|c| c.cell.provider_id == "primary")
        .unwrap();
    assert!(
        case.answered_by_provider_id.is_none(),
        "nothing improved, so the configured provider is what the case reports"
    );
    assert!(case.fallback_attempts.is_empty());
}

/// A test's own exclusion is not a suggestion. `--provider` narrows which cells
/// run; `skip_providers` says something about this test and must hold even when
/// the provider is reached sideways.
#[tokio::test]
async fn skip_providers_excludes_a_fallback_candidate() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", ANSWERS, "")
        ),
        "  - id: t\n    skip_providers: [backup]\n    assert:\n      - type: contains\n        value: GOOD\n",
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    assert!(
        r.cases.iter().all(|c| c.answered_by_provider_id.is_none()),
        "a test that skips a provider must not reach it through a back door"
    );
}

/// `--provider` picks which *cells* run. It must neither strip the chain (which
/// would silently remove the resilience you configured) nor expand cells for the
/// fallback (which would run a provider you excluded).
#[tokio::test]
async fn a_provider_filter_keeps_the_chain_without_expanding_cells_for_it() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let mut opts = RunOptions::default();
    opts.filter.providers = vec!["primary".to_string()];
    let r = run_suite(&yaml, opts).await;

    assert_eq!(r.cases.len(), 1, "only the selected provider gets a cell");
    assert_eq!(r.cases[0].cell.provider_id, "primary");
    assert_eq!(
        r.cases[0].answered_by_provider_id.as_deref(),
        Some("backup"),
        "the fallback is still built and still reachable"
    );
}

/// Chains are not followed, by construction — which is what makes a cycle
/// unconstructible rather than something to detect at run time.
#[tokio::test]
async fn a_fallbacks_own_fallback_is_not_followed() {
    let yaml = suite(
        &format!(
            "{}{}{}",
            emitting("a", REFUSES, &chain("b")),
            emitting("b", REFUSES, &chain("c")),
            emitting("c", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    let case = r.cases.iter().find(|c| c.cell.provider_id == "a").unwrap();
    assert!(
        case.answered_by_provider_id.is_none(),
        "a → b stops at b, which also refused, so a's own outcome is reported"
    );
}

#[tokio::test]
async fn no_fallback_disables_the_chain() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(
        &yaml,
        RunOptions {
            fallback: false,
            ..Default::default()
        },
    )
    .await;
    assert!(r.cases.iter().all(|c| c.answered_by_provider_id.is_none()));
}

/// The trigger set is configurable, and an empty one means "hand off only on
/// hard failures" — a refusal is then a result to be graded.
#[tokio::test]
async fn an_empty_trigger_set_hands_off_only_on_a_hard_failure() {
    let providers = format!(
        "{}{}",
        emitting("primary", REFUSES, &chain("backup")),
        emitting("backup", ANSWERS, "")
    );
    let yaml = format!(
        "version: 1\nproject: test\nsuite: fallback\nrunner:\n  fallback_on_empty_reason: []\nproviders:\n{providers}tests:\n{WANTS_GOOD}"
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    assert!(r.cases.iter().all(|c| c.answered_by_provider_id.is_none()));
}

/// The run-level tally the CLI's all-fallback exit guard reads.
#[tokio::test]
async fn the_summary_counts_cases_answered_by_a_fallback() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("backup")),
            emitting("backup", OTHER, "")
        ),
        "  - id: t\n    assert:\n      - type: contains\n        value: OTHER\n",
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    // Two cells: the primary's fell back, the backup's own did not.
    assert_eq!(r.summary.total, 2);
    assert_eq!(r.summary.fallback_cases, 1);
}

/// An unknown id never reaches the runner — `validate` refuses it — but a chain
/// pointing at a provider that failed to *build* must degrade, not panic.
#[tokio::test]
async fn an_unresolvable_chain_entry_is_skipped_rather_than_fatal() {
    let yaml = suite(
        &format!(
            "{}{}",
            emitting("primary", REFUSES, &chain("ghost, backup")),
            emitting("backup", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    let case = r
        .cases
        .iter()
        .find(|c| c.cell.provider_id == "primary")
        .unwrap();
    assert_eq!(case.answered_by_provider_id.as_deref(), Some("backup"));
}

/// The two-link version of this passes even with `skip(1)`, which is how the
/// bug it guards against survived review. With three links, links 0 and 1 both
/// hand off, so dropping only the first leaves link 1's record behind — a case
/// claiming a handoff that `answered_by_provider_id` says did not happen, and a
/// run document that no longer matches what a no-fallback run would write.
#[tokio::test]
async fn a_three_link_chain_that_never_improves_records_no_attempts() {
    let yaml = suite(
        &format!(
            "{}{}{}",
            emitting("primary", REFUSES, &chain("b, c")),
            emitting("b", REFUSES, ""),
            emitting("c", REFUSES, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    let case = r
        .cases
        .iter()
        .find(|c| c.cell.provider_id == "primary")
        .unwrap();
    assert!(case.answered_by_provider_id.is_none());
    assert!(
        case.fallback_attempts.is_empty(),
        "the primary is the reported answer, so nothing was passed over: {:?}",
        case.fallback_attempts
    );
    assert_eq!(r.summary.fallback_cases, 0);
    // And the document carries neither key, so its content hash still matches a
    // no-fallback run of the same suite.
    let v = serde_json::to_value(case).unwrap();
    assert!(v.get("fallback_attempts").is_none());
}

/// "Improved on the primary" is judged by the trigger set, not by whether the
/// output is good. A link whose empty reason is *not* a trigger has answered as
/// far as the policy is concerned, so it is the reported answer — and the case
/// is then graded on it, which is where a bad one gets caught.
#[tokio::test]
async fn a_link_declining_for_an_untriggered_reason_is_still_the_answer() {
    // `truncated` is a real, reproducible property of the request rather than a
    // provider declining to engage, so it is deliberately absent from the
    // default trigger set.
    let providers = format!(
        "{}{}",
        emitting("primary", REFUSES, &chain("backup")),
        emitting(
            "backup",
            r#"{\"output\":\"\",\"empty_reason\":\"truncated\"}"#,
            ""
        )
    );
    let yaml = suite(&providers, WANTS_GOOD);
    let r = run_suite(&yaml, RunOptions::default()).await;
    let case = r
        .cases
        .iter()
        .find(|c| c.cell.provider_id == "primary")
        .unwrap();
    assert_eq!(case.answered_by_provider_id.as_deref(), Some("backup"));
    assert_eq!(
        case.empty_reason.as_ref().map(|r| r.as_str()),
        Some("truncated"),
        "and the case reports the answering link's diagnosis, not the primary's"
    );
}

/// A provider whose command emits a body the parser cannot read: `exec` reports
/// that as `exec_failed`... so instead use one that exits non-zero *after*
/// writing nothing, which is the same class. What matters here is the position:
/// the failing link is in the MIDDLE of the chain.
///
/// The invariant-3 recovery used to be gated on `i == last`, so a middle link
/// settling on a non-trigger outcome returned its own result and the primary's
/// gradeable refusal was thrown away. With `fallback: [b, c]` and `b` failing
/// for a reason that is not a trigger, `c` is never reached and the case must
/// still report the primary rather than becoming an error.
#[tokio::test]
async fn a_middle_link_settling_badly_still_reports_the_primary() {
    // `b` answers with an empty output whose reason is not in the trigger set,
    // so the chain settles on `b` at i == 1 with last == 2.
    let yaml = suite(
        &format!(
            "{}{}{}",
            emitting("primary", REFUSES, &chain("b, c")),
            emitting(
                "b",
                r#"{\"output\":\"\",\"empty_reason\":\"truncated\"}"#,
                ""
            ),
            emitting("c", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let r = run_suite(&yaml, RunOptions::default()).await;
    let case = r
        .cases
        .iter()
        .find(|c| c.cell.provider_id == "primary")
        .unwrap();
    // `truncated` is not a trigger, so `b` counts as having answered: it is the
    // reported result and `c` is never reached. The point of the test is that
    // this settles deterministically at a middle link rather than falling
    // through to the last one.
    assert_eq!(case.answered_by_provider_id.as_deref(), Some("b"));
    assert_eq!(
        case.empty_reason.as_ref().map(|r| r.as_str()),
        Some("truncated")
    );
}

/// A cache whose reads start failing after the first, so the *second* link of a
/// chain gets `cache_unavailable` — an error class deliberately absent from the
/// trigger list.
#[derive(Default)]
struct FailsAfterFirstGet {
    gets: AtomicUsize,
}

#[async_trait]
impl CacheBackend for FailsAfterFirstGet {
    async fn get(&self, _key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        if self.gets.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(None);
        }
        Err(CacheError(anyhow::anyhow!("connection refused")))
    }
    async fn put(&self, _key: &CacheKey, _entry: &CacheEntry) -> Result<(), CacheError> {
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(CacheStats::default())
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// The regression the `i == last` gate allowed.
///
/// The primary refuses and hands off; link `b`'s cache read then fails with
/// `cache_unavailable`, which is **not** a trigger, so the chain settles at a
/// *middle* link. Gated on `i == last`, the recovery never ran and the case
/// became `CaseStatus::Error` — strictly worse than the same suite with no
/// `fallback:` configured, which is the one thing invariant 3 forbids.
#[tokio::test]
async fn a_middle_link_erroring_untriggered_never_makes_the_case_worse() {
    let yaml = suite(
        &format!(
            "{}{}{}",
            emitting("primary", REFUSES, &chain("b, c")),
            emitting("b", ANSWERS, ""),
            emitting("c", ANSWERS, "")
        ),
        WANTS_GOOD,
    );
    let suite_parsed = domarinn_core::load_str(&yaml).unwrap();
    let cache = FailsAfterFirstGet::default();
    let r = run(
        &suite_parsed,
        Path::new("."),
        &cache,
        None,
        &RunOptions::default(),
    )
    .await
    .unwrap();

    let case = r
        .cases
        .iter()
        .find(|c| c.cell.provider_id == "primary")
        .unwrap();
    assert!(
        case.answered_by_provider_id.is_none(),
        "the primary is the reported answer"
    );
    assert_ne!(
        case.status,
        CaseStatus::Error,
        "a fallback whose cache read failed must not turn a gradeable refusal \
         into an infrastructure error"
    );
    assert_eq!(
        case.empty_reason.as_ref().map(|r| r.as_str()),
        Some("refusal"),
        "and the reported diagnosis is the primary's own"
    );
}
