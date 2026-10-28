//! Refill-on-read token buckets keyed by an arbitrary string.
//!
//! Shared by the MCP `tools/call` limiter and the per-client login limiter.
//! Hand-rolled rather than pulling in `tower-governor`: neither caller keys on
//! something a `tower` layer can see (one keys on the resolved identity, the
//! other on the resolved client address), and the mechanics are short.
//!
//! **Hard-bounded.** The keys come from the network, so an attacker choosing
//! fresh ones must not be able to grow the map without limit — not even when
//! every bucket is mid-drain and none is idle enough to sweep.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// A set of token buckets. No background task: a bucket is advanced lazily
/// when it is next consulted.
pub(crate) struct TokenBuckets {
    buckets: Mutex<HashMap<String, Bucket>>,
    capacity: f64,
    refill_per_sec: f64,
    max_tracked: usize,
}

impl TokenBuckets {
    pub(crate) fn new(per_minute: u32, burst: u32, max_tracked: usize) -> Self {
        TokenBuckets {
            buckets: Mutex::new(HashMap::new()),
            capacity: f64::from(burst.max(1)),
            refill_per_sec: f64::from(per_minute.max(1)) / 60.0,
            max_tracked: max_tracked.max(1),
        }
    }

    /// Take one token for `key`. `Err(retry_after)` when its bucket is empty.
    pub(crate) fn take(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());

        if buckets.len() >= self.max_tracked && !buckets.contains_key(key) {
            // A bucket back at capacity carries no state worth keeping.
            let capacity = self.capacity;
            let refill = self.refill_per_sec;
            buckets.retain(|_, b| {
                refilled(b.tokens, b.last, now, refill, capacity) < capacity - f64::EPSILON
            });
            if buckets.len() >= self.max_tracked {
                evict_fullest(&mut buckets, self.max_tracked, now, refill, capacity);
            }
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

    #[cfg(test)]
    fn len(&self) -> usize {
        self.buckets.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// Drop the fullest tenth of the buckets. Every bucket is mid-drain, so
/// something with state must go; the fullest are the closest to a fresh bucket
/// and so the cheapest to forget, while a drained bucket — the one actually
/// doing the limiting — survives. Freeing a batch rather than one slot means
/// the O(n) selection is paid once per `max_tracked / 10` new keys, not on
/// every one.
fn evict_fullest(
    buckets: &mut HashMap<String, Bucket>,
    max_tracked: usize,
    now: Instant,
    refill: f64,
    capacity: f64,
) {
    let batch = (max_tracked / 10).max(1).min(buckets.len());
    let mut levels: Vec<(f64, &String)> = buckets
        .iter()
        .map(|(key, b)| (refilled(b.tokens, b.last, now, refill, capacity), key))
        .collect();
    // Fullest first; only the partition point matters, not the order within.
    levels.select_nth_unstable_by(batch - 1, |a, b| b.0.total_cmp(&a.0));
    let doomed: Vec<String> = levels[..batch]
        .iter()
        .map(|(_, key)| (*key).clone())
        .collect();
    for key in doomed {
        buckets.remove(&key);
    }
}

fn refilled(tokens: f64, last: Instant, now: Instant, per_sec: f64, capacity: f64) -> f64 {
    let elapsed = now.saturating_duration_since(last).as_secs_f64();
    (tokens + elapsed * per_sec).min(capacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_is_allowed_then_the_bucket_empties() {
        let buckets = TokenBuckets::new(60, 3, 16);
        let now = Instant::now();
        for i in 0..3 {
            assert!(buckets.take("k", now).is_ok(), "call {i} should pass");
        }
        assert!(buckets.take("k", now).is_err(), "burst+1 should be limited");
    }

    #[test]
    fn tokens_refill_over_time() {
        let buckets = TokenBuckets::new(60, 2, 16);
        let start = Instant::now();
        assert!(buckets.take("k", start).is_ok());
        assert!(buckets.take("k", start).is_ok());
        assert!(buckets.take("k", start).is_err());
        // 60/min = 1/s, so one second buys exactly one call.
        assert!(buckets.take("k", start + Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn buckets_are_independent() {
        let buckets = TokenBuckets::new(60, 1, 16);
        let now = Instant::now();
        assert!(buckets.take("a", now).is_ok());
        assert!(buckets.take("a", now).is_err());
        assert!(buckets.take("b", now).is_ok());
    }

    #[test]
    fn retry_after_is_at_least_one_second() {
        let buckets = TokenBuckets::new(60, 1, 16);
        let now = Instant::now();
        assert!(buckets.take("k", now).is_ok());
        let wait = buckets.take("k", now).unwrap_err();
        assert!(wait >= Duration::from_secs(1));
    }

    #[test]
    fn the_map_never_exceeds_its_bound_even_when_no_bucket_is_full() {
        // Every key has just spent a token, so none is back at capacity and
        // the idle sweep alone frees nothing.
        let buckets = TokenBuckets::new(60, 3, 100);
        let now = Instant::now();
        for i in 0..1_000 {
            assert!(buckets.take(&format!("k{i}"), now).is_ok());
            assert!(
                buckets.len() <= 100,
                "{} tracked after key {i}",
                buckets.len()
            );
        }
    }

    #[test]
    fn eviction_prefers_the_fullest_buckets() {
        let buckets = TokenBuckets::new(60, 3, 10);
        let now = Instant::now();
        for _ in 0..3 {
            assert!(buckets.take("hot", now).is_ok());
        }
        for i in 0..9 {
            assert!(buckets.take(&format!("warm{i}"), now).is_ok());
        }
        // The map is full; every newcomer forces an eviction.
        for i in 0..20 {
            assert!(buckets.take(&format!("new{i}"), now).is_ok());
            assert!(buckets.len() <= 10);
        }
        // Forgetting the drained bucket would hand it a fresh burst.
        assert!(
            buckets.take("hot", now).is_err(),
            "hot must still be limited"
        );
    }
}
