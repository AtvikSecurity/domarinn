//! Adopting cache entries written under a previous key shape.
//!
//! # Why this exists
//!
//! A cache key is a SHA-256 over the provider fingerprint plus the rendered
//! request. When a fingerprint's *shape* changes, every key derived from it
//! changes with it, and a store full of perfectly good responses becomes
//! unreachable — not wrong, just invisible. For an LLM-graded suite that is paid
//! for in real money, so a shape change should cost one migration rather than
//! one re-run of everything anybody has ever cached.
//!
//! An entry cannot be re-keyed offline. It records the fingerprint that produced
//! it but not the *request*, and the key hashes both, so there is nothing to
//! recompute from and no inverting SHA-256. What can be done is to migrate at
//! the only moment both halves are in hand: a lookup. On a miss, [`runner_cache`]
//! re-derives the key from each historical fingerprint this provider would once
//! have published, and the first one that hits is adopted — returned as a hit and
//! written under the current key, so the next run finds it directly.
//!
//! [`runner_cache`]: crate::runner::runner_cache
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
//! This module is disposable by construction and should be deleted one release
//! after the last shape it lists stops appearing in the wild. Nothing else may
//! depend on it: the shapes below are frozen historical literals, and editing
//! one to match a current fingerprint would defeat the point.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use serde_json::Value as Json;

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

/// Every `exec` fingerprint domarinn has published, newest first.
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

    fn command(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
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

    /// A legacy shape must never coincide with the current one, or a probe would
    /// re-read the key that just missed and the migration would be a no-op that
    /// costs a lookup.
    #[test]
    fn no_legacy_shape_equals_the_current_fingerprint() {
        use crate::provider::Provider;
        let p = crate::exec_provider::ExecProvider::new(
            "p",
            command(&["./sut"]),
            Default::default(),
            None,
            Some("v1".into()),
            None,
        );
        let current = crate::cache::canonical_json(&p.fingerprint());
        for legacy in p.legacy_fingerprints() {
            assert_ne!(
                crate::cache::canonical_json(legacy),
                current,
                "a legacy shape collided with the current fingerprint"
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
