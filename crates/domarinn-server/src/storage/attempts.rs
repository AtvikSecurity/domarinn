//! Login-attempt counters and the server's self-minted secrets.
//!
//! Throttling lives in the runs database, not in process memory, for two
//! reasons: a restart must not hand an attacker a fresh guessing budget, and
//! every replica behind a load balancer must count against the same one. Each
//! throttle key (an account, or a device cookie) is one `login_attempts` row.
//!
//! An attempt is *reserved* before the caller verifies the password, and a
//! successful login clears the key. Counting first is what stops N concurrent
//! guesses from all reading "9 failures so far" and all slipping through: the
//! reservation is one upsert inside one write transaction, so the increments
//! serialise on the row (SQLite's single writer; Postgres's row lock, held
//! until commit, across replicas too).

use rand::Rng;

use super::exec::{params, Queryable};
use super::{now_ms, Storage};

/// How long reserved attempts count against a key's budget. Shared by every
/// policy, which is what lets [`Storage::prune_login_attempts`] decide a row
/// is dead without a per-row expiry column.
pub const ATTEMPT_WINDOW_MS: i64 = 15 * 60 * 1000;

/// How long after the first lock of a chain later locks keep escalating. A
/// key that stays quiet this long starts again at the base duration.
pub const LOCKOUT_ESCALATION_MS: i64 = 24 * 60 * 60 * 1000;

/// One throttle's limits: how many attempts a window allows, and the lock
/// ladder that follows (doubling from `lock_base_ms`, capped at
/// `lock_cap_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptPolicy {
    pub max_failures: i64,
    pub lock_base_ms: i64,
    pub lock_cap_ms: i64,
}

/// Whether the caller may go on to verify a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptVerdict {
    /// An attempt was reserved; verify, and clear the key on success.
    Allowed,
    /// The key is locked. Nothing was reserved.
    Locked { retry_after_ms: i64 },
}

/// The duration of the `lock_ordinal`-th lock (1-based) in an escalation
/// chain. The shift is clamped at 20 so a key that keeps tripping for days
/// cannot shift the base out of `i64` and wrap into a short or negative lock;
/// the cap is reached long before 2^20 times the base anyway.
pub fn lock_duration_ms(lock_ordinal: i64, policy: AttemptPolicy) -> i64 {
    let shift = (lock_ordinal - 1).clamp(0, 20) as u32;
    policy
        .lock_base_ms
        .saturating_mul(1i64 << shift)
        .min(policy.lock_cap_ms)
}

impl Storage {
    /// Reserve one login attempt against `key`, or report that it is locked.
    /// See the module docs for why the attempt is counted before the password
    /// is checked.
    pub async fn begin_login_attempt(
        &self,
        key: String,
        now_ms: i64,
        policy: AttemptPolicy,
    ) -> anyhow::Result<AttemptVerdict> {
        self.runs
            .write(move |conn| {
                let mut tx = conn.immediate_tx()?;
                // A locked key keeps its count (a locked attempt reserves
                // nothing); a lapsed window restarts at this attempt. The
                // window only moves when the key is not locked, so a lock
                // outliving its window does not reset the count under it.
                let (failures, locked_until, lockouts, lockout_window_start): (
                    i64,
                    i64,
                    i64,
                    i64,
                ) = tx.query_row(
                    "INSERT INTO login_attempts (key, failures, window_start) VALUES (?1, 1, ?2)
                     ON CONFLICT (key) DO UPDATE SET
                       failures = CASE
                           WHEN login_attempts.locked_until > ?2 THEN login_attempts.failures
                           WHEN login_attempts.window_start + ?3 <= ?2 THEN 1
                           ELSE login_attempts.failures + 1 END,
                       window_start = CASE
                           WHEN login_attempts.locked_until <= ?2
                                AND login_attempts.window_start + ?3 <= ?2 THEN ?2
                           ELSE login_attempts.window_start END
                     RETURNING failures, locked_until, lockouts, lockout_window_start",
                    &params![key, now_ms, ATTEMPT_WINDOW_MS],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;

                if locked_until > now_ms {
                    tx.commit()?;
                    return Ok(AttemptVerdict::Locked {
                        retry_after_ms: locked_until - now_ms,
                    });
                }
                if failures <= policy.max_failures {
                    tx.commit()?;
                    return Ok(AttemptVerdict::Allowed);
                }

                // This attempt trips the lock. Within a day of the chain's
                // first lock each lock doubles; after a quiet day the chain
                // restarts. The count resets so the key gets a full budget
                // once the lock lapses — the escalation is the deterrent.
                let chain_expired =
                    lockouts == 0 || lockout_window_start + LOCKOUT_ESCALATION_MS <= now_ms;
                let ordinal = if chain_expired { 1 } else { lockouts + 1 };
                let chain_start = if ordinal == 1 {
                    now_ms
                } else {
                    lockout_window_start
                };
                let until = now_ms + lock_duration_ms(ordinal, policy);
                tx.execute(
                    "UPDATE login_attempts
                     SET failures = 0, window_start = ?2, locked_until = ?3,
                         lockouts = ?4, lockout_window_start = ?5
                     WHERE key = ?1",
                    &params![key, now_ms, until, ordinal, chain_start],
                )?;
                tx.commit()?;
                Ok(AttemptVerdict::Locked {
                    retry_after_ms: until - now_ms,
                })
            })
            .await
    }

    /// Forget every attempt against `key` (after a successful login).
    pub async fn clear_login_attempts(&self, key: String) -> anyhow::Result<()> {
        self.runs
            .write(move |conn| {
                conn.execute("DELETE FROM login_attempts WHERE key = ?1", &params![key])?;
                Ok(())
            })
            .await
    }

    /// Delete rows that no longer affect any verdict: not locked, outside the
    /// attempt window, and either never locked or past the escalation window.
    /// A row inside its escalation window must survive even with a zero
    /// count, or pruning it would reset the next lock to the base duration.
    pub async fn prune_login_attempts(&self, now_ms: i64) -> anyhow::Result<u64> {
        self.runs
            .write(move |conn| {
                conn.execute(
                    "DELETE FROM login_attempts
                     WHERE locked_until <= ?1
                       AND window_start + ?2 <= ?1
                       AND (lockouts = 0 OR lockout_window_start + ?3 <= ?1)",
                    &params![now_ms, ATTEMPT_WINDOW_MS, LOCKOUT_ESCALATION_MS],
                )
            })
            .await
    }

    /// The named server secret as 64 hex characters, minted (32 random bytes)
    /// on first use. `ON CONFLICT DO NOTHING` then a read makes concurrent
    /// first calls — including from other replicas — agree on whichever
    /// insert won, instead of each caching its own key.
    pub async fn server_secret(&self, name: String) -> anyhow::Result<String> {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let candidate = hex::encode(bytes);
        self.runs
            .write(move |conn| {
                conn.execute(
                    "INSERT INTO server_secrets (name, value, created_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (name) DO NOTHING",
                    &params![name, candidate, now_ms()],
                )?;
                conn.query_row(
                    "SELECT value FROM server_secrets WHERE name = ?1",
                    &params![name],
                    |row| row.get(0),
                )
            })
            .await
    }
}
