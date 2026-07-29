//! Adopting cache entries written under a previous key shape.
//!
//! # Why this exists
//!
//! Through 0.4.x a provider cache key was a SHA-256 over the provider
//! *fingerprint* plus the pieces of the request — the seven-part composite
//! [`legacy_provider_key`] still computes. 0.5.0 replaced it with one rule for
//! every cached call: a hash of the canonical outgoing request
//! ([`crate::cache_key::request_cache_key`]). Every key in every store moved,
//! and a store full of perfectly good responses became unreachable — not wrong,
//! just invisible. For an LLM-graded suite that is paid for in real money, so a
//! key change should cost one migration rather than one re-run of everything
//! anybody has ever cached.
//!
//! An entry cannot be re-keyed offline. A ≤0.4.x entry records the fingerprint
//! that produced it but not the *request*, and the key hashes both, so there is
//! nothing to recompute from and no inverting SHA-256. What can be done is to
//! migrate at the only moment both halves are in hand: a lookup. On a miss,
//! [`runner_cache`] re-derives the old key from each fingerprint this provider
//! would once have published, and the first one that hits is adopted — returned
//! as a hit and rewritten under the current key (carrying its canonical request
//! this time), so the next run finds it directly.
//!
//! [`runner_cache`]: crate::runner::runner_cache
//!
//! # The shapes this machinery knows
//!
//! All of them are [`legacy_provider_key`] over one of these fingerprints:
//!
//! | Provider | Shapes | Shipped |
//! |---|---|---|
//! | `openai` | `{type, model, base_url, params}` | ≤0.4.0 |
//! | `anthropic` | `{type, model, base_url, params}` | ≤0.4.0 |
//! | `http` | `{type, url, method, body, output_expr, headers?}` | ≤0.4.0 |
//! | `exec` | `{type, command, env, cache_salt}`, then four older generations | ≤0.4.0, then 0.3.1 and earlier (see [`legacy_exec_fingerprints`]) |
//!
//! The ≤0.4.0 shape of each is, by construction, whatever
//! [`crate::provider::Provider::fingerprint`] returns today: that method is
//! frozen for exactly this reason (it also feeds `digests::provider_digest`,
//! which `--against` diffing compares), so
//! [`crate::provider::Provider::legacy_fingerprints`] leads with it rather than
//! duplicating four literals that could drift apart from the pin tests guarding
//! them. The older `exec` generations have no such source and are literals here.
//! `golden_*_key` below pins the exact key each shape produces, so a refactor
//! that moves one fails loudly rather than quietly stranding a store.
//!
//! # Cost, and why it is bounded
//!
//! Probing costs one extra `get` per historical shape per miss, which against a
//! remote backend is latency nobody asked for on a cache that has nothing to
//! migrate. [`MigrationProbe`] therefore spends a small budget of cases and
//! stops if none of them adopt anything: an empty or already-migrated store pays
//! a few dozen lookups once, and a store worth migrating keeps probing for as
//! long as it keeps paying off.
//!
//! # Deleting this
//!
//! This module is disposable by construction: it ships in 0.5.0, stays through
//! 0.6.x, and is deleted in 0.7.0 — by which point a store still holding only
//! ≤0.4.x entries has gone two minor releases without a run. Nothing else may
//! depend on it: everything here is a frozen historical literal, and editing one
//! to match a current shape would defeat the point.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use serde_json::Value as Json;

use crate::cache::CacheKey;
use crate::provider::ProviderRequest;

/// The provider cache key as domarinn computed it through 0.4.x.
///
/// **Frozen ≤0.4.x shape — do not edit.** Every byte of this function is what a
/// store in the wild was keyed with; changing it does not migrate anything, it
/// just makes the probe miss. It was `cache_key::provider_cache_key` until 0.5.0
/// moved the live path onto [`crate::cache_key::request_cache_key`], and it is
/// preserved verbatim apart from the rename.
///
/// Seven members at most: `fingerprint`, `prompt`, `vars`, `params`, `repeat`
/// unconditionally, plus `tools` and `case_salt` only when set. That conditional
/// discipline is load-bearing history — canonical JSON emits every member that
/// is present, so a null member hashes differently from an absent one, and an
/// entry written before `tools` or `case_salt` existed keeps its key only
/// because they are inserted rather than defaulted.
pub fn legacy_provider_key(fingerprint: &Json, req: &ProviderRequest, repeat: u32) -> CacheKey {
    let prompt = req
        .prompt
        .as_ref()
        .map(|p| serde_json::to_value(p).unwrap_or(Json::Null))
        .unwrap_or(Json::Null);
    let mut parts = serde_json::json!({
        "fingerprint": fingerprint,
        "prompt": prompt,
        "vars": Json::Object(req.vars.clone().into_iter().collect()),
        "params": Json::Object(req.params.clone()),
        "repeat": repeat,
    });
    // Same "only when present" discipline as `case_salt` below, and for the
    // same reason: every entry written before tools existed must keep its key.
    // An empty list is not a declaration, it is the absence of one.
    if !req.tools.is_empty() {
        parts.as_object_mut().expect("json! object literal").insert(
            "tools".to_string(),
            serde_json::to_value(&req.tools).unwrap_or(Json::Null),
        );
    }
    // Only when set. An empty salt is a real value and is deliberately not
    // normalized to "unset".
    if let Some(salt) = &req.case_salt {
        parts
            .as_object_mut()
            .expect("json! object literal")
            .insert("case_salt".to_string(), Json::String(salt.clone()));
    }
    CacheKey::compute(&parts)
}

/// Cases allowed to probe for legacy entries before giving up, when none of
/// them has adopted anything.
///
/// Small because the common case is a store with nothing to migrate, where
/// every probe is pure waste. Not zero because a cold local disk makes the
/// probes nearly free and the payoff — a warm shared cache surviving an upgrade
/// — is worth several orders of magnitude more than the lookups.
const PROBE_BUDGET: i64 = 8;

/// Whether a run should still look for entries under a previous key shape.
///
/// Shared across the whole run, so the budget is spent globally rather than per
/// provider: one adopted entry anywhere is evidence the store is worth reading,
/// and no adoptions after a handful of cases is evidence it is not.
#[derive(Debug)]
pub struct MigrationProbe {
    remaining: AtomicI64,
    adopted_any: AtomicBool,
    enabled: bool,
}

impl MigrationProbe {
    /// A probe that spends [`PROBE_BUDGET`] cases looking for something to adopt.
    pub fn new() -> Self {
        MigrationProbe {
            remaining: AtomicI64::new(PROBE_BUDGET),
            adopted_any: AtomicBool::new(false),
            enabled: true,
        }
    }

    /// A probe that never fires — `--no-cache-migration`, and the default for
    /// embedders that have no legacy store to read.
    pub fn disabled() -> Self {
        MigrationProbe {
            remaining: AtomicI64::new(0),
            adopted_any: AtomicBool::new(false),
            enabled: false,
        }
    }

    /// Claim the right to probe for one case. False once the budget is spent
    /// with nothing to show for it.
    pub fn should_probe(&self) -> bool {
        if !self.enabled {
            return false;
        }
        // Once anything has been adopted the store has clearly earned the
        // lookups, so stop counting.
        if self.adopted_any.load(Ordering::Relaxed) {
            return true;
        }
        self.remaining.fetch_sub(1, Ordering::Relaxed) > 0
    }

    /// Record that a probe found and adopted an entry.
    pub fn record_adoption(&self) {
        self.adopted_any.store(true, Ordering::Relaxed);
    }

    /// Whether this run adopted anything, so the caller can say so once at the
    /// end rather than per entry.
    pub fn adopted_any(&self) -> bool {
        self.adopted_any.load(Ordering::Relaxed)
    }
}

impl Default for MigrationProbe {
    fn default() -> Self {
        MigrationProbe::new()
    }
}

/// The `exec` fingerprints domarinn published *before* 0.4.0, newest first.
///
/// The ≤0.4.0 shape is not here: it is the provider's own current
/// [`crate::provider::Provider::fingerprint`], which
/// [`crate::provider::Provider::legacy_fingerprints`] puts in front of this
/// list. These four have no such source and are frozen literals.
///
/// Ordered newest-first so the most likely match is found in the fewest
/// lookups. Each entry is a frozen historical literal — see the module docs; do
/// not "fix" one to track the current fingerprint.
///
/// | Shape | Shipped in | Differs by |
/// |---|---|---|
/// | `{type, command, program: [{path, content}], env, cache_salt}` | unreleased | program keyed on file contents |
/// | `{type, command, program: [{path, len, mtime, mtime_ns}], env, cache_salt}` | 0.3.1 | program keyed on stat metadata |
/// | `{type, command, program: […], cache_salt}` | 0.3.0 | before `env` joined the key |
/// | `{type, command, cache_salt}` | 0.2.x | before `program` existed at all |
///
/// The two `program` flavours are both produced from one filesystem walk. The
/// `mtime` ones only ever match on the machine that wrote them, with the files
/// untouched since — which is exactly the case that matters, a developer or a
/// CI runner upgrading in place.
pub fn legacy_exec_fingerprints(
    command: &[String],
    env_digest: Option<&str>,
    cache_salt: Option<&str>,
    base_dir: Option<&Path>,
) -> Vec<Json> {
    let (by_content, by_stat) = legacy_programs(command, base_dir);
    let mut out = Vec::with_capacity(4);
    for program in [&by_content, &by_stat] {
        out.push(serde_json::json!({
            "type": "exec",
            "command": command,
            "program": program,
            "env": env_digest,
            "cache_salt": cache_salt,
        }));
    }
    out.push(serde_json::json!({
        "type": "exec",
        "command": command,
        "program": by_stat,
        "cache_salt": cache_salt,
    }));
    out.push(serde_json::json!({
        "type": "exec",
        "command": command,
        "cache_salt": cache_salt,
    }));
    out
}

/// The `program` array as both historical flavours computed it, in one walk.
///
/// Returns `(contents, stat)`. Both are `[]` when nothing resolves, which is
/// what the old code produced for `docker run …` — and back then that meant
/// "not cacheable", so such a provider has no legacy entries to find anyway.
fn legacy_programs(command: &[String], base_dir: Option<&Path>) -> (Json, Json) {
    // Matches the historical cap. Above it the old code fell back to stat
    // metadata for *both* flavours, which is why the fallback is shared here.
    const MAX_HASHED_BYTES: u64 = 256 * 1024 * 1024;

    let (mut contents, mut stat) = (Vec::new(), Vec::new());
    for (i, arg) in command.iter().enumerate() {
        let Some(resolved) = crate::exec::resolve_program_arg(arg, i == 0, base_dir) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&resolved) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
        let by_stat = serde_json::json!({
            "path": arg,
            "len": meta.len(),
            "mtime": mtime.map(|d| d.as_secs()),
            "mtime_ns": mtime.map(|d| d.subsec_nanos()),
        });
        let hashed = (meta.len() <= MAX_HASHED_BYTES)
            .then(|| file_digest(&resolved))
            .flatten();
        contents.push(match hashed {
            Some(content) => serde_json::json!({ "path": arg, "content": content }),
            None => by_stat.clone(),
        });
        stat.push(by_stat);
    }
    (Json::Array(contents), Json::Array(stat))
}

/// blake3 of a file's contents, in the `blake3:<hex>` spelling the historical
/// `program` array used.
fn file_digest(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("blake3:{}", hasher.finalize().to_hex()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Provider, TestMeta};
    use std::collections::BTreeMap;

    fn command(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    // ── The frozen 7-part key ────────────────────────────────────────────────
    //
    // These moved here with `provider_cache_key` when 0.5.0 took the live path
    // off it. They are no longer describing a design; they are describing a
    // store in the wild, so the ones that survived the move are the ones whose
    // failure would mean a stranded cache.

    fn req(var: &str) -> ProviderRequest {
        salted(var, None)
    }

    fn salted(var: &str, case_salt: Option<&str>) -> ProviderRequest {
        let mut vars = BTreeMap::new();
        vars.insert("x".to_string(), Json::String(var.into()));
        ProviderRequest {
            tools: Vec::new(),
            prompt: None,
            vars,
            params: serde_json::Map::new(),
            test: TestMeta::default(),
            case_salt: case_salt.map(String::from),
        }
    }

    fn fp() -> Json {
        serde_json::json!({"type": "exec"})
    }

    /// The golden literal for the frozen shape itself.
    ///
    /// Every other test in this file compares two keys, so all of them would
    /// still pass if the whole composite moved — which is precisely the failure
    /// that strands a store, and it is silent. This one is a magic constant on
    /// purpose: it is the key a 0.4.x domarinn wrote for these inputs, and if it
    /// changes, the migration reads nothing.
    #[test]
    fn golden_seven_part_key() {
        assert_eq!(
            legacy_provider_key(&fp(), &req("a"), 0).0,
            "sha256:0f1db1256de263796a24c8e28cdc00f746a3b633e53a9757fffb66089d4f7fc5"
        );
    }

    /// The same, per provider kind, over the fingerprint each one published in
    /// ≤0.4.0 — the shape [`crate::provider::Provider::legacy_fingerprints`]
    /// leads with. A change to any provider's `fingerprint()` breaks its own pin
    /// test *and* this one, which is the point: the pin says "the shape is
    /// stable", this says "and the store that shape keyed is still reachable".
    #[test]
    fn golden_key_per_provider_kind() {
        let openai = crate::openai::OpenAiProvider::new("p", "gpt-x", None, None, None, None);
        let anthropic =
            crate::anthropic::AnthropicProvider::new("p", "claude-x", None, None, None, None);
        let http = crate::http_provider::HttpProvider::new(
            "p",
            "https://sut.test/generate",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let exec = crate::exec_provider::ExecProvider::new(
            "p",
            command(&["./sut"]),
            Default::default(),
            None,
            Some("v1".into()),
            None,
        );

        for (kind, provider) in [
            ("openai", &openai as &dyn Provider),
            ("anthropic", &anthropic),
            ("http", &http),
            ("exec", &exec),
        ] {
            let key = legacy_provider_key(&provider.fingerprint(), &req("a"), 0);
            let expected = match kind {
                "openai" => {
                    "sha256:3eab215ad9714bb3de737c39ee2ffd4bc10a6ca3559827e12f3d51752bc65ba5"
                }
                "anthropic" => {
                    "sha256:201a83d6a3f05e9ff211272aa914f29a5dffb7adb6ce062e8636c98f5d62965f"
                }
                "http" => "sha256:b0e9e3d55ddcd90bfc8d74ac77b584be21f5fb1c1126eeca5af74db285769e5a",
                _ => "sha256:8f4c04d03bce936e39fab440d6dab66fe54dda15566670b212dd5e804ff51124",
            };
            assert_eq!(key.0, expected, "{kind}: the ≤0.4.x key moved");
        }
    }

    /// The load-bearing backward-compatibility rule of the old shape: an
    /// unsalted case hashed exactly like the pre-`case_salt` object, because the
    /// member was inserted rather than defaulted to null. An entry written
    /// before the field existed is reachable only while this holds.
    #[test]
    fn the_conditional_members_are_absent_rather_than_null() {
        let before_case_salt = CacheKey::compute(&serde_json::json!({
            "fingerprint": fp(),
            "prompt": Json::Null,
            "vars": {"x": "a"},
            "params": {},
            "repeat": 0,
        }));
        assert_eq!(legacy_provider_key(&fp(), &req("a"), 0), before_case_salt);

        // And the same for `tools`: an empty declaration is the absence of one.
        let mut empty_tools = req("a");
        empty_tools.tools = Vec::new();
        assert_eq!(
            legacy_provider_key(&fp(), &empty_tools, 0),
            before_case_salt
        );
    }

    /// A set salt separates, an empty one is a real value, and neither is
    /// normalized away.
    #[test]
    fn a_case_salt_separates_and_an_empty_one_is_not_unset() {
        assert_ne!(
            legacy_provider_key(&fp(), &salted("a", None), 0),
            legacy_provider_key(&fp(), &salted("a", Some("d1")), 0)
        );
        assert_ne!(
            legacy_provider_key(&fp(), &salted("a", Some("d1")), 0),
            legacy_provider_key(&fp(), &salted("a", Some("d2")), 0)
        );
        assert_ne!(
            legacy_provider_key(&fp(), &salted("a", Some("")), 0),
            legacy_provider_key(&fp(), &salted("a", None), 0)
        );
    }

    /// Declaring a tool moved the old key too — so an entry written by a suite
    /// with `tools:` is only adoptable if that stays true.
    #[test]
    fn declared_tools_moved_the_key() {
        let with_tools = |names: &[&str]| {
            let mut r = req("a");
            r.tools = names
                .iter()
                .map(|n| crate::config::ToolDef {
                    name: (*n).to_string(),
                    description: None,
                    input_schema: None,
                })
                .collect();
            r
        };
        assert_ne!(
            legacy_provider_key(&fp(), &req("a"), 0),
            legacy_provider_key(&fp(), &with_tools(&["get_weather"]), 0)
        );
        assert_ne!(
            legacy_provider_key(&fp(), &with_tools(&["get_weather"]), 0),
            legacy_provider_key(&fp(), &with_tools(&["get_weather", "get_time"]), 0)
        );
    }

    /// The test id never entered the old key, so two cases with identical vars
    /// shared one ≤0.4.x entry — the shape of an `exec` suite whose system under
    /// test resolves its own prompt from the test id. Recorded because adoption
    /// inherits it: those two cases still share the *new* entry the first of
    /// them re-files.
    #[test]
    fn the_test_id_was_never_keyed() {
        let mut a = req("same");
        a.test = TestMeta {
            id: "case-a".into(),
            tags: vec![],
        };
        let mut b = req("same");
        b.test = TestMeta {
            id: "case-b".into(),
            tags: vec![],
        };
        assert_eq!(
            legacy_provider_key(&fp(), &a, 0),
            legacy_provider_key(&fp(), &b, 0)
        );
    }

    /// The point of the ordering: the shape most likely to be present is probed
    /// first, so a store written by the previous release is found in one lookup.
    #[test]
    fn shapes_are_ordered_newest_first() {
        let fps = legacy_exec_fingerprints(&command(&["./sut"]), None, Some("v1"), None);
        assert_eq!(fps.len(), 4);
        assert!(fps[0].get("program").is_some() && fps[0].get("env").is_some());
        assert!(fps[2].get("program").is_some() && fps[2].get("env").is_none());
        assert!(fps[3].get("program").is_none());
    }

    /// …and the newest shape of all is the one 0.4.0 shipped: the provider's own
    /// current fingerprint. Every provider now has at least that one, where
    /// before 0.5.0 the three network kinds had no history at all — their key
    /// had never moved, so there was nothing to adopt. Now it has.
    #[test]
    fn the_current_fingerprint_leads_the_probe_list() {
        let exec = crate::exec_provider::ExecProvider::new(
            "p",
            command(&["./sut"]),
            Default::default(),
            None,
            Some("v1".into()),
            None,
        );
        let shapes = exec.legacy_fingerprints();
        assert_eq!(
            shapes.len(),
            5,
            "the 0.4.0 shape plus four older generations"
        );
        assert_eq!(shapes[0], exec.fingerprint());
        assert_eq!(
            shapes[1..],
            legacy_exec_fingerprints(&command(&["./sut"]), None, Some("v1"), None)[..]
        );

        let anthropic =
            crate::anthropic::AnthropicProvider::new("p", "claude-x", None, None, None, None);
        assert_eq!(
            anthropic.legacy_fingerprints(),
            vec![anthropic.fingerprint()]
        );
    }

    /// No legacy key may equal the live key for the same call, or a probe would
    /// re-read the key that just missed and the migration would be a no-op that
    /// costs a lookup.
    ///
    /// Retargeted in 0.5.0: the old version compared *fingerprints*, which was
    /// the right question while the fingerprint was half the key. Now the live
    /// key hashes the canonical request instead, so the honest comparison is
    /// key-to-key — and it holds structurally, since `request` is not a member of
    /// the frozen object and `fingerprint` is not a member of the live one.
    #[test]
    fn no_legacy_key_equals_the_live_key_for_one_call() {
        // A checkout where `./sut` resolves, because that is what separates the
        // two `program` flavours: with nothing on disk both walk to `[]` and the
        // 0.3.1 and 0.3.0 shapes collapse into one key. That costs a redundant
        // lookup for a command naming nothing readable — which under the old
        // rules was never cacheable, so it has nothing to adopt anyway.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sut"), "#!/bin/sh\necho v1").unwrap();
        let p = crate::exec_provider::ExecProvider::new(
            "p",
            command(&["./sut"]),
            Default::default(),
            None,
            Some("v1".into()),
            Some(dir.path()),
        );
        let request = p.canonical_request(&req("a")).expect("exec is cacheable");
        let live = crate::cache_key::request_cache_key(&request, 0, p.cache_salt(), None);
        let mut seen = std::collections::HashSet::new();
        for fingerprint in p.legacy_fingerprints() {
            let key = legacy_provider_key(&fingerprint, &req("a"), 0);
            assert_ne!(key, live, "a legacy key collided with the live one");
            assert!(
                seen.insert(key.0.clone()),
                "two legacy shapes produced one key, so a probe is wasted"
            );
        }
    }

    /// Both flavours are produced from one walk, and a file that resolves
    /// contributes to both — the stat one is what a 0.3.1 store keyed on.
    #[test]
    fn a_resolvable_file_appears_in_both_flavours() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sut"), "#!/bin/sh\necho v1").unwrap();

        let (contents, stat) = legacy_programs(&command(&["./sut"]), Some(dir.path()));
        assert_eq!(contents.as_array().unwrap().len(), 1);
        assert_eq!(stat.as_array().unwrap().len(), 1);
        assert!(contents[0].get("content").is_some(), "{contents}");
        assert!(stat[0].get("mtime").is_some(), "{stat}");
    }

    /// A command naming nothing readable produces empty arrays. Such a provider
    /// was not cacheable under the old rules, so there is nothing to adopt.
    ///
    /// The program name is deliberately one no machine has. An earlier draft
    /// used `docker`, which is on `PATH` on plenty of developer machines and on
    /// most CI images — so the test asserted "resolves to nothing" about
    /// something that resolves.
    #[test]
    fn an_unresolvable_command_yields_empty_programs() {
        let (contents, stat) = legacy_programs(
            &command(&["definitely-not-a-real-binary-xyz", "run", "img"]),
            None,
        );
        assert_eq!(contents, Json::Array(vec![]));
        assert_eq!(stat, Json::Array(vec![]));
    }

    /// The budget is spent per case and stops a pointless probe loop, but one
    /// adoption buys unlimited further probing.
    #[test]
    fn the_probe_budget_stops_when_nothing_is_adopted() {
        let probe = MigrationProbe::new();
        for _ in 0..PROBE_BUDGET {
            assert!(probe.should_probe());
        }
        assert!(!probe.should_probe(), "budget must run out");

        let probe = MigrationProbe::new();
        assert!(probe.should_probe());
        probe.record_adoption();
        for _ in 0..(PROBE_BUDGET * 10) {
            assert!(probe.should_probe(), "an adoption lifts the budget");
        }
    }

    #[test]
    fn a_disabled_probe_never_fires() {
        let probe = MigrationProbe::disabled();
        assert!(!probe.should_probe());
        assert!(!probe.adopted_any());
    }
}
