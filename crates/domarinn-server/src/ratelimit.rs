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
    /// Full sweeps run, so tests can prove they are amortised.
    #[cfg(test)]
    sweeps: std::sync::atomic::AtomicUsize,
}

impl TokenBuckets {
    pub(crate) fn new(per_minute: u32, burst: u32, max_tracked: usize) -> Self {
        TokenBuckets {
            buckets: Mutex::new(HashMap::new()),
            capacity: f64::from(burst.max(1)),
            refill_per_sec: f64::from(per_minute.max(1)) / 60.0,
            max_tracked: max_tracked.max(1),
            #[cfg(test)]
            sweeps: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Take one token for `key`. `Err(retry_after)` when its bucket is empty.
    pub(crate) fn take(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());

        if buckets.len() >= self.max_tracked && !buckets.contains_key(key) {
            #[cfg(test)]
            self.sweeps
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // A bucket back at capacity carries no state worth keeping.
            let capacity = self.capacity;
            let refill = self.refill_per_sec;
            buckets.retain(|_, b| {
                refilled(b.tokens, b.last, now, refill, capacity) < capacity - f64::EPSILON
            });
            // Every sweep leaves at least a batch of free slots, however few
            // buckets the idle pass found. Stopping at "one free" would make
            // the next new key pay the O(n) scan again under the mutex, and
            // fresh keys (a /64 each) are the attacker's to mint.
            let target = self.max_tracked - eviction_batch(self.max_tracked);
            if buckets.len() > target {
                let excess = buckets.len() - target;
                evict_fullest(&mut buckets, excess, now, refill, capacity);
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

    #[cfg(test)]
    fn sweeps(&self) -> usize {
        self.sweeps.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// How many slots a sweep leaves free: a tenth of the bound, at least one.
/// Never more than `max_tracked`, so the subtraction in `take` cannot wrap.
fn eviction_batch(max_tracked: usize) -> usize {
    (max_tracked / 10).max(1)
}

/// Drop the `count` fullest buckets. Every bucket left is mid-drain, so
/// something with state must go; the fullest are the closest to a fresh bucket
/// and so the cheapest to forget, while a drained bucket — the one actually
/// doing the limiting — survives. Freeing a batch rather than one slot means
/// the O(n) selection is paid once per `max_tracked / 10` new keys, not on
/// every one.
fn evict_fullest(
    buckets: &mut HashMap<String, Bucket>,
    count: usize,
    now: Instant,
    refill: f64,
    capacity: f64,
) {
    let batch = count.min(buckets.len());
    if batch == 0 {
        return;
    }
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

    /// Every key mid-drain, so only eviction frees slots: N new keys past the
    /// bound cost at most N / batch sweeps, not one each.
    #[test]
    fn sweeps_are_amortised_over_a_batch_of_new_keys() {
        let buckets = TokenBuckets::new(60, 3, 100);
        let now = Instant::now();
        for i in 0..100 {
            assert!(buckets.take(&format!("fill{i}"), now).is_ok());
        }
        assert_eq!(buckets.sweeps(), 0);
        for i in 0..1_000 {
            assert!(buckets.take(&format!("new{i}"), now).is_ok());
        }
        assert!(
            buckets.sweeps() <= 1_000 / eviction_batch(100),
            "{} sweeps for 1000 new keys",
            buckets.sweeps()
        );
    }

    /// The case the batch used to miss: the idle pass frees a few slots but
    /// fewer than a batch. The sweep must still top up to a full batch free.
    #[test]
    fn a_sweep_leaves_at_least_a_batch_free_even_when_a_few_were_idle() {
        let buckets = TokenBuckets::new(60, 3, 100);
        let start = Instant::now();
        for i in 0..3 {
            assert!(buckets.take(&format!("idle{i}"), start).is_ok());
        }
        // Long enough for the idle three to refill, not for anyone else.
        let now = start + Duration::from_secs(60);
        for i in 0..97 {
            assert!(buckets.take(&format!("busy{i}"), now).is_ok());
        }
        assert_eq!(buckets.len(), 100);
        assert!(buckets.take("newcomer", now).is_ok());
        assert_eq!(buckets.sweeps(), 1);
        assert_eq!(buckets.len(), 100 - eviction_batch(100) + 1);
    }

    #[test]
    fn a_bound_of_one_keeps_one_bucket() {
        let buckets = TokenBuckets::new(60, 1, 1);
        let now = Instant::now();
        assert!(buckets.take("a", now).is_ok());
        assert!(buckets.take("a", now).is_err());
        assert!(buckets.take("b", now).is_ok(), "b evicts a");
        assert_eq!(buckets.len(), 1);
        assert!(buckets.take("b", now).is_err(), "b's own state is kept");
        assert!(buckets.take("a", now).is_ok(), "a was forgotten");
        assert_eq!(buckets.len(), 1);
    }

    /// A bucket that has refilled to capacity is the default state, so it
    /// goes before one that is half spent, which would come back with a
    /// fresh burst if forgotten.
    #[test]
    fn a_full_idle_bucket_is_evicted_before_a_half_drained_one() {
        let buckets = TokenBuckets::new(60, 4, 2);
        let start = Instant::now();
        assert!(buckets.take("idle", start).is_ok());
        let now = start + Duration::from_secs(5);
        assert!(buckets.take("half", now).is_ok());
        assert!(buckets.take("half", now).is_ok());
        // Map full; "idle" has refilled to 4, "half" sits at 2.
        assert!(buckets.take("new", now).is_ok());
        assert_eq!(buckets.len(), 2);
        assert!(buckets.take("half", now).is_ok());
        assert!(buckets.take("half", now).is_ok());
        assert!(
            buckets.take("half", now).is_err(),
            "half must keep its spent tokens"
        );
    }
}
