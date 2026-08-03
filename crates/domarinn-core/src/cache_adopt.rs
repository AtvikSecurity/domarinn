//! The budget that bounds probing for cache entries under a previous key shape.
//!
//! Lives here rather than in [`crate::cache_migrate`] because that module is
//! disposable by design — it declares its own deletion release, and everything
//! in it is a frozen historical literal. This is neither. Every migration
//! domarinn will ever run needs a way to stop looking, so the mechanism outlives
//! any particular history it is spent on.
//!
//! Two spaces are probed out of this one budget today: the =<0.4.x
//! fingerprint-keyed entries [`crate::cache_migrate`] owns, and the pre-0.8.0
//! canonical requests that carried a full `base_url` in their `url` member (see
//! [`crate::provider::Provider::legacy_canonical_requests`]). One budget rather
//! than one each, because the question they answer is the same — is this store
//! old enough to be worth extra lookups — and the first adoption from either
//! answers it for both.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// Cases allowed to probe for legacy entries before giving up, when none of
/// them has adopted anything.
///
/// Small because the common case is a store with nothing to migrate, where
/// every probe is pure waste. Not zero because a cold local disk makes the
/// probes nearly free and the payoff — a warm shared cache surviving an upgrade
/// — is worth several orders of magnitude more than the lookups.
pub(crate) const PROBE_BUDGET: i64 = 8;

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
