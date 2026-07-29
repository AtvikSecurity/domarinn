//! A per-identity token bucket for `tools/call`.
//!
//! The spec makes rate limiting a MUST for tool invocations, and the rest of
//! the server has none. Hand-rolled rather than pulling in `tower-governor`:
//! it is sixty lines, and a `tower` layer could not key on the identity the
//! auth middleware resolves.
//!
//! **Keyed on identity, not IP.** [`crate::serve`] uses `axum::serve` without
//! `into_make_service_with_connect_info`, so `ConnectInfo<SocketAddr>` is not
//! available — and would not exist under the `oneshot` test harness either.
//! One consequence worth knowing: every holder of a given static token shares
//! a single bucket, because a static token's only identity is its scope label.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::auth::Identity;

/// Sustained `tools/call` rate per identity.
pub const DEFAULT_PER_MINUTE: u32 = 60;
/// How many calls may arrive back-to-back before the sustained rate applies.
pub const DEFAULT_BURST: u32 = 10;
/// Cap on tracked identities before idle buckets are swept.
const MAX_TRACKED: usize = 4096;

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// A refill-on-read token bucket. No background task: buckets are advanced
/// lazily when they are next consulted.
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    capacity: f64,
    refill_per_sec: f64,
}

impl RateLimiter {
    pub fn new(per_minute: u32, burst: u32) -> RateLimiter {
        RateLimiter {
            buckets: Mutex::new(HashMap::new()),
            capacity: f64::from(burst.max(1)),
            refill_per_sec: f64::from(per_minute.max(1)) / 60.0,
        }
    }

    /// Take one token for `identity`. `Err(retry_after)` when the bucket is
    /// empty.
    pub fn check(&self, identity: &Identity) -> Result<(), Duration> {
        self.take(&bucket_key(identity), Instant::now())
    }

    fn take(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());

        if buckets.len() >= MAX_TRACKED && !buckets.contains_key(key) {
            // A bucket back at capacity carries no state worth keeping.
            let capacity = self.capacity;
            let refill = self.refill_per_sec;
            buckets.retain(|_, b| {
                refilled(b.tokens, b.last, now, refill, capacity) < capacity - f64::EPSILON
            });
        }

        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: self.capacity,
            last: now,
        });
        bucket.tokens = refilled(
            bucket.tokens,
            bucket.last,
            now,
            self.refill_per_sec,
            self.capacity,
        );
        bucket.last = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }
        // Round up so a client that honors `Retry-After` always succeeds.
        let deficit = 1.0 - bucket.tokens;
        Err(Duration::from_secs_f64(
            (deficit / self.refill_per_sec).ceil().max(1.0),
        ))
    }
}

impl Default for RateLimiter {
    fn default() -> RateLimiter {
        RateLimiter::new(DEFAULT_PER_MINUTE, DEFAULT_BURST)
    }
}

fn refilled(tokens: f64, last: Instant, now: Instant, per_sec: f64, capacity: f64) -> f64 {
    let elapsed = now.saturating_duration_since(last).as_secs_f64();
    (tokens + elapsed * per_sec).min(capacity)
}

/// The bucket an identity falls into. Anonymous callers share one bucket,
/// which is the point: they are indistinguishable to us.
fn bucket_key(identity: &Identity) -> String {
    if let Some(user_id) = &identity.user_id {
        return format!("user:{}", user_id.as_str());
    }
    match &identity.label {
        Some(label) => format!("label:{label}"),
        None => "anonymous".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_is_allowed_then_the_bucket_empties() {
        let limiter = RateLimiter::new(60, 3);
        let now = Instant::now();
        for i in 0..3 {
            assert!(limiter.take("k", now).is_ok(), "call {i} should pass");
        }
        assert!(limiter.take("k", now).is_err(), "burst+1 should be limited");
    }

    #[test]
    fn tokens_refill_over_time() {
        let limiter = RateLimiter::new(60, 2);
        let start = Instant::now();
        assert!(limiter.take("k", start).is_ok());
        assert!(limiter.take("k", start).is_ok());
        assert!(limiter.take("k", start).is_err());
        // 60/min = 1/s, so one second buys exactly one call.
        assert!(limiter.take("k", start + Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn buckets_are_independent() {
        let limiter = RateLimiter::new(60, 1);
        let now = Instant::now();
        assert!(limiter.take("a", now).is_ok());
        assert!(limiter.take("a", now).is_err());
        assert!(limiter.take("b", now).is_ok());
    }

    #[test]
    fn retry_after_is_at_least_one_second() {
        let limiter = RateLimiter::new(60, 1);
        let now = Instant::now();
        assert!(limiter.take("k", now).is_ok());
        let wait = limiter.take("k", now).unwrap_err();
        assert!(wait >= Duration::from_secs(1));
    }

    #[test]
    fn anonymous_identities_share_a_bucket() {
        let anon = Identity::anonymous(crate::AuthMode::Closed);
        assert_eq!(bucket_key(&anon), "anonymous");
    }

    #[test]
    fn a_user_backed_identity_gets_its_own_bucket() {
        let mut identity = Identity::anonymous(crate::AuthMode::Closed);
        identity.user_id = Some(crate::domain::UserId::new("u1"));
        identity.label = Some("root".to_string());
        // The account id wins over the display label.
        assert_eq!(bucket_key(&identity), "user:u1");
    }
}
