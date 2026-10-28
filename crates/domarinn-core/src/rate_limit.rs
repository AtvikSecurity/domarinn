//! `runner.rate_limit`: a run-wide cap on outgoing provider requests.
//!
//! One limiter per run, shared by every cell and every provider, and consulted
//! once per *attempt* — so a retry spends a slot just as the first try did, and
//! so does a `fallback:` link. It sits behind the cache: a hit never reaches it,
//! because a replayed answer is not a request anybody's quota sees.
//!
//! Grader and embedding calls are **not** limited here. They go to a different
//! endpoint, usually a different vendor, and through their own path
//! (`request_cache`); throttling them against the system under test's budget
//! would slow a run down without protecting anything.
//!
//! The shape is the simplest thing that honours "N per second": requests are
//! spaced `1 / rps` apart with no burst allowance. A burst would let the first
//! `concurrency` cells of every run land in the same instant, which is exactly
//! the traffic shape a provider's limiter rejects. No dependency: a mutex over
//! the next free slot, and `tokio::time` to wait for it — which also means a
//! test can drive it with paused time instead of sleeping.

use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// The longest gap the limiter will impose between two requests.
///
/// A ceiling rather than an error because `validate` already refuses a
/// non-positive rate, and anything below one request a day is not a rate limit
/// anyone meant — it is a typo that would otherwise overflow an `Instant`.
const MAX_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Spaces calls at least `1 / rps` apart, across every task that shares it.
#[derive(Debug)]
pub struct RateLimiter {
    interval: Duration,
    /// The earliest instant the next caller may proceed. `None` until the first
    /// call, so the first request of a run is never delayed.
    next: Mutex<Option<Instant>>,
}

impl RateLimiter {
    /// A limiter admitting `rps` requests per second, or `None` when `rps` is
    /// not a positive, finite number.
    pub fn per_second(rps: f64) -> Option<Self> {
        if !(rps.is_finite() && rps > 0.0) {
            return None;
        }
        let interval = Duration::try_from_secs_f64(1.0 / rps)
            .unwrap_or(MAX_INTERVAL)
            .min(MAX_INTERVAL);
        Some(RateLimiter {
            interval,
            next: Mutex::new(None),
        })
    }

    /// The gap this limiter enforces between consecutive requests.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Wait for this caller's slot.
    ///
    /// Slots are handed out in the order callers arrive, and reserved before
    /// the wait: a caller that is cancelled mid-wait forfeits its slot rather
    /// than letting the next one in early, which errs on the side of the
    /// provider's quota.
    pub async fn acquire(&self) {
        let slot = {
            let mut next = self.next.lock().expect("rate limiter mutex");
            let now = Instant::now();
            let slot = match *next {
                Some(n) if n > now => n,
                _ => now,
            };
            *next = Some(slot + self.interval);
            slot
        };
        tokio::time::sleep_until(slot).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_positive_or_non_finite_rate_builds_no_limiter() {
        for rps in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(RateLimiter::per_second(rps).is_none(), "rps = {rps}");
        }
    }

    #[test]
    fn the_interval_is_the_reciprocal_of_the_rate() {
        assert_eq!(
            RateLimiter::per_second(4.0).unwrap().interval(),
            Duration::from_millis(250)
        );
        assert_eq!(
            RateLimiter::per_second(0.5).unwrap().interval(),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn an_absurdly_low_rate_is_capped_rather_than_overflowing() {
        assert_eq!(
            RateLimiter::per_second(1e-300).unwrap().interval(),
            MAX_INTERVAL
        );
    }

    /// Paused time: the clock only moves when every task is idle, so the
    /// elapsed time below is exactly what the limiter imposed.
    #[tokio::test(start_paused = true)]
    async fn concurrent_callers_are_spaced_one_interval_apart() {
        let limiter = std::sync::Arc::new(RateLimiter::per_second(2.0).unwrap());
        let start = Instant::now();
        let tasks: Vec<_> = (0..5)
            .map(|_| {
                let limiter = limiter.clone();
                tokio::spawn(async move {
                    limiter.acquire().await;
                    Instant::now() - start
                })
            })
            .collect();
        let mut admitted = Vec::new();
        for t in tasks {
            admitted.push(t.await.unwrap());
        }
        admitted.sort();
        let expected: Vec<Duration> = (0..5).map(|i| Duration::from_millis(500 * i)).collect();
        assert_eq!(
            admitted, expected,
            "first is immediate, then one per 500 ms"
        );
    }

    /// An idle limiter does not bank credit: after a long pause the next call
    /// goes immediately, and the one after it still waits a full interval.
    #[tokio::test(start_paused = true)]
    async fn idle_time_is_not_banked_as_a_burst() {
        let limiter = RateLimiter::per_second(1.0).unwrap();
        limiter.acquire().await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        let t0 = Instant::now();
        limiter.acquire().await;
        assert_eq!(Instant::now() - t0, Duration::ZERO);
        limiter.acquire().await;
        assert_eq!(Instant::now() - t0, Duration::from_secs(1));
    }
}
