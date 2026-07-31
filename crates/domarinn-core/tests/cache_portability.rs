//! A cache key is a property of the question, not of the machine that asks it.
//!
//! Everything here runs the *same suite* from two simulated machines — different
//! absolute paths, different file timestamps, different working directories,
//! different ambient environments — against one shared store, and asserts the
//! second run pays nothing. That is the property the S3 and results-server
//! backends exist to sell, and for `exec` providers it did not hold: the
//! fingerprint hashed the program's own bytes, so a fresh clone or a CI runner
//! that compiled its own provider never matched anything anyone else had
//! written.
//!
//! The negatives matter just as much and live at the bottom. A key that never
//! misses is not portable, it is broken — so every ingredient that *should* move
//! it gets a test proving it still does.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::RunResult;

/// In-memory, first-write-wins cache mirroring the real backends' semantics.
#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
}

impl MemCache {
    fn entries(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

#[async_trait]
impl CacheBackend for MemCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
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
        Ok(CacheStats {
            entries: self.entries() as u64,
            ..Default::default()
        })
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A checkout of one suite: a program on disk plus the YAML that names it.
///
/// The whole point is that two of these, at different paths with different
/// timestamps, are interchangeable to the cache. `program` is written as a
/// relative `./sut` so the suite text is byte-identical between them — an
/// absolute path would differ per machine and put the difference in `command`,
/// where it genuinely belongs and would mask what is being tested.
struct Checkout {
    dir: tempfile::TempDir,
}

impl Checkout {
    fn new(program_body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let checkout = Checkout { dir };
        checkout.write_program(program_body);
        checkout
    }

    fn write_program(&self, body: &str) {
        let path = self.path().join("sut");
        std::fs::write(&path, format!("cat >/dev/null\n{body}\n")).unwrap();
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Backdate the program far enough that no "recent checkout" heuristic could
    /// accidentally match — the exact shape `git clone` produces, where every
    /// file is stamped with the clone time rather than the commit time.
    fn set_mtime(&self, unix_secs: i64) {
        let path = self.path().join("sut");
        let stamp = format!("@{unix_secs}");
        let ok = std::process::Command::new("touch")
            .args(["-d", &stamp])
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "could not backdate {path:?}");
    }

    fn suite(&self, salt: Option<&str>) -> String {
        suite_yaml("[\"sh\", \"./sut\"]", salt)
    }
}

fn suite_yaml(command: &str, salt: Option<&str>) -> String {
    let salt_line = salt
        .map(|s| format!("\n    cache_salt: \"{s}\""))
        .unwrap_or_default();
    format!(
        r#"
version: 1
project: test
suite: portability
providers:
  - id: p
    type: exec
    command: {command}{salt_line}
tests:
  - id: case-a
    vars: {{x: "a"}}
  - id: case-b
    vars: {{x: "b"}}
"#
    )
}

/// Run `yaml` with `base_dir` as the suite directory, against `cache`.
async fn run_in(yaml: &str, base_dir: &Path, cache: &dyn CacheBackend) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    run(&suite, base_dir, cache, None, &RunOptions::default())
        .await
        .unwrap()
}

/// The shared assertion: a second machine pays nothing and writes nothing.
async fn assert_second_machine_is_free(first: &Checkout, second: &Checkout, salt: Option<&str>) {
    let cache = MemCache::default();

    let cold = run_in(&first.suite(salt), first.path(), &cache).await;
    assert_eq!(cold.summary.cache_misses, 2, "the first machine pays");
    assert_eq!(cache.entries(), 2);

    let warm = run_in(&second.suite(salt), second.path(), &cache).await;
    assert_eq!(
        warm.summary.cache_hits, 2,
        "the second machine must reuse every entry"
    );
    assert_eq!(
        cache.entries(),
        2,
        "and must not write a single new one — a new entry means a new key"
    );
}

// ── Portability: the same question keys the same way anywhere ────────────────

#[tokio::test]
async fn a_different_checkout_path_does_not_bust() {
    // Two clones of one repo, byte-identical, in different directories. This is
    // the ordinary shape of two developers, or a developer and CI.
    let body = "printf '{\"output\":\"ok\"}'";
    assert_second_machine_is_free(&Checkout::new(body), &Checkout::new(body), None).await;
}

#[tokio::test]
async fn mtime_alone_does_not_bust() {
    // Byte-identical program, rewritten so its mtime moves. `git` does not
    // record mtime, so a fresh checkout re-stamps every file — which is why
    // keying on it meant no two machines ever agreed.
    let body = "printf '{\"output\":\"ok\"}'";
    let first = Checkout::new(body);
    let second = Checkout::new(body);
    second.write_program(body); // same bytes, new timestamp
    assert_second_machine_is_free(&first, &second, None).await;
}

#[tokio::test]
async fn a_backdated_mtime_does_not_bust() {
    // 1990, well outside any plausible clock skew.
    let body = "printf '{\"output\":\"ok\"}'";
    let first = Checkout::new(body);
    let second = Checkout::new(body);
    second.set_mtime(631_152_000);
    assert_second_machine_is_free(&first, &second, None).await;
}

#[tokio::test]
async fn a_rebuilt_program_still_hits() {
    // The deliberate trade, asserted rather than left implicit. Two CI runners
    // compiling identical source produce different bytes — Rust builds are not
    // byte-reproducible — and used to share nothing at all. `cache_salt` is how
    // a suite says a rebuild *should* re-run; without one, a rebuild is a
    // warning, not an invalidation.
    let first = Checkout::new("printf '{\"output\":\"ok\"}'");
    let second = Checkout::new("printf '{\"output\":\"ok\"}' # a different build");
    assert_second_machine_is_free(&first, &second, None).await;
}

#[tokio::test]
async fn differing_ambient_env_does_not_bust() {
    // A variable the suite never declares must not reach the key, or no two
    // shells would agree. (The child does inherit it — that hazard is real and
    // documented; the answer is to declare it in `env:` or pass `${env:VAR}`,
    // both of which *are* keyed. See the negatives below.)
    let body = "printf '{\"output\":\"ok\"}'";
    let first = Checkout::new(body);
    let second = Checkout::new(body);
    let cache = MemCache::default();

    std::env::set_var("DOMARINN_PORTABILITY_PROBE", "machine-one");
    run_in(&first.suite(None), first.path(), &cache).await;
    std::env::set_var("DOMARINN_PORTABILITY_PROBE", "machine-two");
    let warm = run_in(&second.suite(None), second.path(), &cache).await;
    std::env::remove_var("DOMARINN_PORTABILITY_PROBE");

    assert_eq!(warm.summary.cache_hits, 2);
    assert_eq!(cache.entries(), 2);
}

#[tokio::test]
async fn the_working_directory_does_not_bust() {
    // `base_dir` decides where a relative command resolves. It used to decide
    // whether the program was *found*, and therefore whether it contributed to
    // the key — so the same suite run from a repo root and from its own
    // directory produced two different keys for one question.
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    let cache = MemCache::default();

    run_in(&checkout.suite(None), checkout.path(), &cache).await;
    // A directory where `./sut` resolves to nothing at all.
    let elsewhere = tempfile::tempdir().unwrap();
    let warm = run_in(&checkout.suite(None), elsewhere.path(), &cache).await;

    assert_eq!(
        warm.summary.cache_hits, 2,
        "a program that cannot be found must key exactly like one that can"
    );
    assert_eq!(cache.entries(), 2);
}

#[tokio::test]
async fn a_salted_provider_is_portable_too() {
    // A salt is a suite-authored constant, so it travels with the YAML. Pinning
    // one must not reintroduce machine-locality.
    let first = Checkout::new("printf '{\"output\":\"ok\"}'");
    let second = Checkout::new("printf '{\"output\":\"ok\"}' # rebuilt");
    second.set_mtime(631_152_000);
    assert_second_machine_is_free(&first, &second, Some("v1")).await;
}

// ── The negatives: what must still miss ──────────────────────────────────────

/// One helper for every "this ingredient must move the key" case: run the
/// `before` suite, then the `after` suite, and require a full miss.
async fn assert_busts(before: &str, after: &str, base_dir: &Path, why: &str) {
    let cache = MemCache::default();
    run_in(before, base_dir, &cache).await;
    let second = run_in(after, base_dir, &cache).await;
    assert_eq!(second.summary.cache_hits, 0, "{why}");
    assert_eq!(second.summary.cache_misses, 2, "{why}");
    assert_eq!(cache.entries(), 4, "{why}");
}

#[tokio::test]
async fn a_changed_salt_busts() {
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    assert_busts(
        &checkout.suite(Some("v1")),
        &checkout.suite(Some("v2")),
        checkout.path(),
        "bumping the version pin is the supported way to discard old answers",
    )
    .await;
}

#[tokio::test]
async fn a_changed_command_busts() {
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    assert_busts(
        &suite_yaml("[\"sh\", \"./sut\"]", None),
        &suite_yaml("[\"sh\", \"./sut\", \"--mode\", \"strict\"]", None),
        checkout.path(),
        "argv names what will answer, so a flag is part of the question",
    )
    .await;
}

/// Two backends behind one wrapper script — the A/B shape that made `env` part
/// of the fingerprint in the first place. `cases` is 1 when the probe *order*
/// matters and 2 for the hit/miss arithmetic `assert_busts` does.
fn endpoint_suite(url: &str, cases: usize) -> String {
    let tests: String = ["a", "b"][..cases]
        .iter()
        .map(|x| format!("  - id: case-{x}\n    vars: {{x: \"{x}\"}}\n"))
        .collect();
    format!(
        r#"
version: 1
project: test
suite: portability
providers:
  - id: p
    type: exec
    command: ["sh", "./sut"]
    env: {{MODEL_ENDPOINT: "{url}"}}
tests:
{tests}"#
    )
}

#[tokio::test]
async fn a_changed_declared_env_busts() {
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    assert_busts(
        &endpoint_suite("http://a", 2),
        &endpoint_suite("http://b", 2),
        checkout.path(),
        "two endpoints must not share entries, or the comparison is fabricated",
    )
    .await;
}

#[tokio::test]
async fn two_declared_env_values_share_no_probe_at_all() {
    // The half `a_changed_declared_env_busts` structurally cannot reach: it
    // populates the cache by *running*, so every entry it lays down is under a
    // live key, and a shape that only ever collides on a legacy key stays
    // invisible to it. Here the question is asked of the lookups themselves.
    //
    // Two of the four historical exec shapes predate `env` joining the key and
    // so cannot carry the digest. Offered to a provider that declares `env`,
    // they recompute identically for every declared value — which on a store
    // carried across versions means pointing the suite at a different endpoint
    // replays the old endpoint's answers, and on a shared tier means writing
    // answers under keys nobody can tell from real ones.
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    let a = keys_probed_for(&endpoint_suite("http://a", 1), checkout.path()).await;
    let b = keys_probed_for(&endpoint_suite("http://b", 1), checkout.path()).await;

    assert_eq!(
        a.len(),
        4,
        "the live key, the ≤0.4.0 shape, and the two older generations that can \
         carry an `env` digest — the other two are withheld: {a:#?}"
    );
    for key in &a {
        assert!(
            !b.contains(key),
            "changing the variable that selects the backend must move every \
             lookup, and {key} is shared"
        );
    }
}

#[tokio::test]
async fn an_interpolated_env_value_busts() {
    // `${env:VAR}` resolves at load time, so the substituted value is inside
    // `command` before the provider is built. This is the keyed way to vary a
    // model from the environment, and the reason it can be recommended.
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    let yaml = suite_yaml(
        "[\"sh\", \"./sut\", \"--model\", \"${env:DOMARINN_PORTABILITY_MODEL}\"]",
        None,
    );
    let cache = MemCache::default();

    std::env::set_var("DOMARINN_PORTABILITY_MODEL", "opus");
    run_in(&yaml, checkout.path(), &cache).await;
    std::env::set_var("DOMARINN_PORTABILITY_MODEL", "sonnet");
    let second = run_in(&yaml, checkout.path(), &cache).await;
    std::env::remove_var("DOMARINN_PORTABILITY_MODEL");

    assert_eq!(
        second.summary.cache_hits, 0,
        "two models sharing a salt must not collide"
    );
    assert_eq!(cache.entries(), 4);
}

#[tokio::test]
async fn changing_one_cases_vars_busts_only_that_case() {
    // Locality, which is what makes any of this worth having: a suite-wide bust
    // is always *correct* and always throws away work somebody paid for.
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    let cache = MemCache::default();
    run_in(&checkout.suite(None), checkout.path(), &cache).await;

    let edited = checkout
        .suite(None)
        .replace(r#"{x: "a"}"#, r#"{x: "EDITED"}"#);
    let second = run_in(&edited, checkout.path(), &cache).await;

    assert_eq!(second.summary.cache_hits, 1, "case-b is untouched");
    assert_eq!(second.summary.cache_misses, 1, "only case-a re-pays");
    assert_eq!(cache.entries(), 3);
}

// ── Adopting entries written under an older key shape ────────────────────────

/// A cache that answers every lookup with a miss and records what was asked for.
///
/// This is how the tests below get hold of a *legacy* key without
/// reconstructing a `ProviderRequest` by hand. A backend never sees the request
/// — only the hash — so hand-building one would mean the test agreeing with
/// itself about the request shape rather than with the runner. Recording the
/// runner's own lookups sidesteps that entirely: on a miss it probes the current
/// key first and then each historical shape in turn, so the keys arrive in a
/// known order and the second one is, by construction, the key an older
/// domarinn would have written under.
#[derive(Default)]
struct KeySpy {
    asked: Mutex<Vec<CacheKey>>,
}

#[async_trait]
impl CacheBackend for KeySpy {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        self.asked.lock().unwrap().push(key.clone());
        Ok(None)
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

/// The keys one cold case triggers: `[current, legacy_newest, …, legacy_oldest]`.
async fn keys_probed_for(yaml: &str, base_dir: &Path) -> Vec<CacheKey> {
    let spy = KeySpy::default();
    run_in(yaml, base_dir, &spy).await;
    let asked = spy.asked.lock().unwrap().clone();
    assert!(
        asked.len() > 2,
        "a cold exec case must probe its historical shapes, got {} lookups",
        asked.len()
    );
    asked
}

/// The `ProviderRequest` the runner builds for a one-case, prompt-less suite
/// whose only var is `x`.
///
/// Spelled out rather than captured because it is the *input* to the frozen key:
/// a test that took the runner's word for it could not tell "the key shape is
/// unchanged" from "the key shape and the reconstruction moved together".
fn request_for(test_id: &str, x: &str) -> domarinn_core::provider::ProviderRequest {
    domarinn_core::provider::ProviderRequest {
        prompt: None,
        vars: [("x".to_string(), serde_json::Value::String(x.to_string()))]
            .into_iter()
            .collect(),
        params: serde_json::Map::new(),
        test: domarinn_core::provider::TestMeta {
            id: test_id.to_string(),
            tags: Vec::new(),
        },
        case_salt: None,
        tools: Vec::new(),
    }
}

#[tokio::test]
async fn an_entry_written_under_a_previous_key_shape_is_adopted() {
    // The upgrade path. Before this, changing the fingerprint's shape stranded
    // every entry in every store — still perfectly good answers, simply
    // unreachable, and re-paid for on the next run. For a shared S3 bucket or a
    // team results server that is the entire value of the thing, discarded by a
    // version bump.
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    // One case, so the probe order is unambiguous.
    let yaml = r#"
version: 1
project: test
suite: portability
providers:
  - id: p
    type: exec
    command: ["sh", "./sut"]
    cache_salt: "v1"
tests:
  - id: case-a
    vars: {x: "a"}
"#
    .to_string();

    let probed = keys_probed_for(&yaml, checkout.path()).await;
    let (current, legacy) = (probed[0].clone(), probed[1].clone());
    assert_ne!(
        current, legacy,
        "a legacy shape must not equal the current one"
    );
    assert_eq!(
        probed.len(),
        6,
        "one live key, then the ≤0.4.0 shape and four older exec generations"
    );
    // The one that matters is the second: it is the key a 0.4.x domarinn wrote
    // under, recomputed from the frozen function and this provider's frozen
    // fingerprint. If these disagree the runner is probing for something nobody
    // ever wrote, and every warm 0.4 store silently re-pays.
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let provider =
        domarinn_core::provider_factory::build_provider(&suite.providers[0], Some(checkout.path()))
            .unwrap();
    assert_eq!(
        legacy,
        domarinn_core::cache_migrate::legacy_provider_key(
            &provider.fingerprint(),
            &request_for("case-a", "a"),
            0
        ),
        "the first probe must be exactly the ≤0.4.x key"
    );

    // Seed only the legacy key. The entry itself is a current-era one filed
    // under the old key rather than a faithful 0.4 artefact — what this test is
    // about is *reachability*, and the key is the whole of that. The full
    // round-trip over a genuinely 0.4-shaped entry (fingerprint, no request) is
    // in `cache_era.rs`.
    let store = MemCache::default();
    let response = {
        let scratch = MemCache::default();
        run_in(&yaml, checkout.path(), &scratch).await;
        let entries: Vec<CacheEntry> = scratch.map.lock().unwrap().values().cloned().collect();
        entries.into_iter().next().unwrap()
    };
    store.put(&legacy, &response).await.unwrap();
    assert_eq!(store.entries(), 1);

    let adopted = run_in(&yaml, checkout.path(), &store).await;
    assert_eq!(
        adopted.summary.cache_hits, 1,
        "an upgrade must adopt what the previous version wrote"
    );
    assert_eq!(
        store.entries(),
        2,
        "and re-file it under the current key, so the probe is paid once"
    );
    assert!(
        store.map.lock().unwrap().contains_key(&current.0),
        "the adopted entry must be reachable under the current key"
    );

    // Settled: the next run finds it directly.
    let settled = run_in(&yaml, checkout.path(), &store).await;
    assert_eq!(settled.summary.cache_hits, 1);
    assert_eq!(store.entries(), 2);
}

#[tokio::test]
async fn cache_only_adopts_rather_than_failing_the_run() {
    // Where migration earns the most. A `--cache-only` run has no live call to
    // fall back on, so without adoption an upgrade turns a warm offline CI job
    // into an infrastructure failure — over answers the store already holds.
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    let yaml = r#"
version: 1
project: test
suite: portability
providers:
  - id: p
    type: exec
    command: ["sh", "./sut"]
    cache_salt: "v1"
tests:
  - id: case-a
    vars: {x: "a"}
"#
    .to_string();

    let probed = keys_probed_for(&yaml, checkout.path()).await;
    let legacy = probed[1].clone();

    // A store holding only what the previous version wrote.
    let store = MemCache::default();
    let response = {
        let scratch = MemCache::default();
        run_in(&yaml, checkout.path(), &scratch).await;
        let entry = scratch.map.lock().unwrap().values().next().unwrap().clone();
        entry
    };
    store.put(&legacy, &response).await.unwrap();

    let suite = domarinn_core::load_str(&yaml).unwrap();
    let strict = run(
        &suite,
        checkout.path(),
        &store,
        None,
        &RunOptions {
            cache_mode: domarinn_core::cache::CacheMode::ReadOnlyStrict,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        strict.summary.cache_hits, 1,
        "the legacy entry must be adopted"
    );
    assert!(
        strict
            .cases
            .iter()
            .all(|c| c.status != domarinn_core::result::CaseStatus::Error),
        "a cache-only run must not fail over an entry it can reach"
    );
}

#[tokio::test]
async fn migration_can_be_turned_off() {
    // The cost of probing is extra lookups, which against a high-latency remote
    // on a store with nothing to migrate is pure waste. `--no-cache-migration`
    // is the opt-out, and it must actually stop the probing rather than just
    // ignoring what it finds.
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    let yaml = checkout.suite(Some("v1"));
    let suite = domarinn_core::load_str(&yaml).unwrap();

    let spy = KeySpy::default();
    let opts = RunOptions {
        cache_migration: false,
        ..Default::default()
    };
    run(&suite, checkout.path(), &spy, None, &opts)
        .await
        .unwrap();

    let asked = spy.asked.lock().unwrap().len();
    assert_eq!(
        asked, 2,
        "two cases, one lookup each, and no historical probing"
    );
}

#[tokio::test]
async fn probing_stops_when_there_is_nothing_to_adopt() {
    // The budget that keeps migration from taxing every future run. A store with
    // nothing to migrate pays for a handful of cases and then stops, rather than
    // multiplying every miss by the number of shapes domarinn has ever had.
    let checkout = Checkout::new("printf '{\"output\":\"ok\"}'");
    let mut tests = String::new();
    for i in 0..40 {
        tests.push_str(&format!("  - id: case-{i}\n    vars: {{x: \"{i}\"}}\n"));
    }
    let yaml = format!(
        r#"
version: 1
project: test
suite: portability
providers:
  - id: p
    type: exec
    command: ["sh", "./sut"]
    cache_salt: "v1"
tests:
{tests}"#
    );

    let spy = KeySpy::default();
    run_in(&yaml, checkout.path(), &spy).await;
    let asked = spy.asked.lock().unwrap().len();

    assert!(
        asked >= 40,
        "every case must still check its own key: {asked} lookups for 40 cases"
    );
    // How many cases actually probed, derived rather than hardcoded: one cold
    // case costs its own key plus every historical shape, so the per-case probe
    // cost is measured from a single-case run of the same provider. Asserting
    // the count of *probing cases* keeps the property ("a handful, not all 40")
    // stable the day a shape is added or retired, where a literal ceiling would
    // quietly become either unreachable or wrong.
    let one_case = yaml.replace(&tests, "  - id: case-0\n    vars: {x: \"0\"}\n");
    let per_case = keys_probed_for(&one_case, checkout.path()).await.len();
    let probed_cases = (asked - 40) / (per_case - 1);
    // The budget in `cache_migrate.rs` is 8 cases, so this is 8 plus a little
    // slack — enough that re-tuning the budget does not mean rewriting this
    // test, tight enough that a budget which stopped stopping still fails.
    assert!(
        probed_cases <= 12,
        "probing must taper off, not run for every case: {probed_cases} of 40 cases \
         probed ({asked} lookups at {per_case} per cold case)"
    );
}
