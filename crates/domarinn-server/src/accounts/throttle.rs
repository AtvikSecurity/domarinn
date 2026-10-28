//! Failed-password throttling for login and the self-service password change.
//!
//! One bounded map of fixed-window counters, keyed per account (password
//! change) or per (client address, username) (login). See [`LoginThrottle`].

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::routes::ApiError;

/// Password attempts allowed per key per [`THROTTLE_WINDOW`].
const MAX_PASSWORD_ATTEMPTS: u32 = 5;
/// How long a key's attempts are remembered, measured from its first one.
const THROTTLE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Cap on tracked keys, so a spray of random usernames cannot grow the map
/// without bound (an entry is ~150 bytes, so the cap is ~15 MiB).
///
/// It is also the size of the job an attacker faces to make the map fail
/// closed (see [`Buckets::begin`]): every slot must hold a *locked* key, which
/// costs [`MAX_PASSWORD_ATTEMPTS`] argon2 verifies apiece, 500k verifies, and
/// must be repeated every [`THROTTLE_WINDOW`] as the locks lapse. At tens of
/// milliseconds a verify and one verify per core at a time, that outruns a
/// typical server's CPU budget for the window.
const MAX_THROTTLE_KEYS: usize = 100_000;

struct Attempts {
    count: u32,
    window_start: Instant,
}

struct Table {
    entries: HashMap<String, Attempts>,
    /// Set when a sweep of a full map found nothing it could drop (every
    /// entry a live lock): the earliest instant one of those locks lapses.
    /// Until then a new key is refused without sweeping again, so a map
    /// pinned full costs O(1) per request, not O(n).
    full_until: Option<Instant>,
}

/// One bounded map of fixed-window attempt counters.
struct Buckets {
    limit: u32,
    capacity: usize,
    table: Mutex<Table>,
}

impl Buckets {
    fn new(limit: u32, capacity: usize) -> Self {
        Self {
            limit,
            capacity,
            table: Mutex::new(Table {
                entries: HashMap::new(),
                full_until: None,
            }),
        }
    }

    /// How many entries one sweep of a full map frees, so the O(n) sweep is
    /// paid once per this many new keys rather than on every one.
    fn sweep_batch(&self) -> usize {
        (self.capacity / 10).max(1)
    }

    fn is_locked(&self, a: &Attempts, now: Instant) -> bool {
        a.count >= self.limit && now.saturating_duration_since(a.window_start) < THROTTLE_WINDOW
    }

    /// Make room for one new key in a full map, or `Err(retry_after)` when
    /// every entry is a live lock.
    ///
    /// Drops every lapsed window, then, if that was not enough, the
    /// least-used live windows (fewest attempts, then oldest) up to
    /// [`Self::sweep_batch`]. A *locked* key is never dropped: evicting it
    /// would let anyone who can mint new keys (spray usernames) wipe a
    /// target's lockout and buy a fresh set of guesses. Dropping fewest-first
    /// also keeps a partly-spent bucket (a target with four failures) behind
    /// the one-attempt entries a spray leaves, so pressure resets nothing
    /// that matters until the spray itself has been evicted.
    fn make_room(&self, table: &mut Table, now: Instant) -> Result<(), Duration> {
        if let Some(until) = table.full_until {
            if now < until {
                return Err(round_up(until - now));
            }
            table.full_until = None;
        }
        table
            .entries
            .retain(|_, a| now.saturating_duration_since(a.window_start) < THROTTLE_WINDOW);
        if table.entries.len() < self.capacity {
            return Ok(());
        }
        let mut unlocked: Vec<(u32, Instant, String)> = table
            .entries
            .iter()
            .filter(|(_, a)| !self.is_locked(a, now))
            .map(|(k, a)| (a.count, a.window_start, k.clone()))
            .collect();
        if unlocked.is_empty() {
            // Every live entry is a lock: fail closed for the new key.
            let earliest = table
                .entries
                .values()
                .map(|a| a.window_start + THROTTLE_WINDOW)
                .min()
                .unwrap_or(now + THROTTLE_WINDOW);
            table.full_until = Some(earliest);
            return Err(round_up(earliest.saturating_duration_since(now)));
        }
        let batch = self.sweep_batch().min(unlocked.len());
        if batch < unlocked.len() {
            unlocked.select_nth_unstable_by_key(batch, |(count, start, _)| (*count, *start));
            unlocked.truncate(batch);
        }
        for (_, _, key) in unlocked {
            table.entries.remove(&key);
        }
        Ok(())
    }

    fn begin(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        if table.entries.len() >= self.capacity && !table.entries.contains_key(key) {
            self.make_room(&mut table, now)?;
        }
        let attempts = table.entries.entry(key.to_string()).or_insert(Attempts {
            count: 0,
            window_start: now,
        });
        let elapsed = now.saturating_duration_since(attempts.window_start);
        if elapsed >= THROTTLE_WINDOW {
            attempts.count = 0;
            attempts.window_start = now;
        }
        if attempts.count >= self.limit {
            return Err(round_up(THROTTLE_WINDOW.saturating_sub(elapsed)));
        }
        attempts.count += 1;
        Ok(())
    }

    fn clear(&self, key: &str) {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        if table.entries.remove(key).is_some() {
            // The map is no longer full of locks.
            table.full_until = None;
        }
    }
}

/// Whole seconds, rounded up and never zero, so a client that honors
/// `Retry-After` always gets in.
fn round_up(wait: Duration) -> Duration {
    Duration::from_secs(wait.as_secs_f64().ceil().max(1.0) as u64)
}

/// In-memory, per-process backoff for password checks: per key (an account,
/// or a client address + username for login), at most
/// [`MAX_PASSWORD_ATTEMPTS`] unsuccessful attempts per [`THROTTLE_WINDOW`],
/// then a 429 with `Retry-After` until the window lapses. A success clears
/// the key.
///
/// Attempts are counted *before* the password is checked, so N concurrent
/// guesses cannot all slip past a check that has not yet seen any of them
/// fail; the successful path then clears them.
///
/// There is deliberately no per-address budget across usernames. One used to
/// sit in front of this, refusing every login from an address after 30
/// failures. Behind a reverse proxy every user shares the proxy's address, so
/// one anonymous client could keep everyone out of password login
/// indefinitely, correct passwords included. Reworked so it never refuses a
/// correct password, it would still have to verify every request and could
/// only turn a wrong-password 401 into a 429 — the caller learns the same
/// verdict either way, so it would bound nothing. What actually bounds the
/// cost of a spray is elsewhere: argon2 CPU and memory by the hashing
/// semaphore, guesses per account by this per-key budget, and memory by
/// [`MAX_THROTTLE_KEYS`]. Rate-limiting a spraying client needs to tell
/// clients apart behind a proxy, which needs a trusted-proxy setting.
///
/// Per-process, like the MCP rate limiter: several replicas behind one load
/// balancer each allow their own budget. That still turns an online guessing
/// attack from unbounded into a handful of tries per quarter hour.
pub(crate) struct LoginThrottle {
    keys: Buckets,
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self {
            keys: Buckets::new(MAX_PASSWORD_ATTEMPTS, MAX_THROTTLE_KEYS),
        }
    }
}

impl LoginThrottle {
    /// Record an attempt for `key`, or `Err(retry_after)` when it is out of
    /// attempts for the current window (or, rarely, when the map is full of
    /// live lockouts and `key` is not among them).
    pub(crate) fn begin(&self, key: &str, now: Instant) -> Result<(), Duration> {
        self.keys.begin(key, now)
    }

    /// Forget `key` after a successful password check.
    pub(crate) fn clear(&self, key: &str) {
        self.keys.clear(key);
    }
}

/// The address part of a login key. IPv6 clients are bucketed by their /64,
/// which is what one host or one customer is normally handed, so cycling
/// through the interface identifier does not mint fresh per-username budgets;
/// IPv4-mapped IPv6 is the IPv4 address it carries.
fn peer_bucket(peer: IpAddr) -> String {
    match peer.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// The throttle key for a login attempt.
///
/// The peer address comes from axum's `ConnectInfo`, which is only present
/// when the server is served with `into_make_service_with_connect_info`; when
/// it is absent (and under the `oneshot` test harness) the key degrades to the
/// username alone. Forwarding headers are deliberately ignored: the server has
/// no trusted-proxy setting, so `X-Forwarded-For` is whatever the attacker
/// says it is, and honoring it would hand every guess a fresh bucket.
pub(crate) fn login_throttle_key(peer: Option<SocketAddr>, username: &str) -> String {
    match peer {
        Some(addr) => format!("login\0{}\0{username}", peer_bucket(addr.ip())),
        None => format!("login\0-\0{username}"),
    }
}

/// "14 minutes", "1 minute", "45 seconds" — rounded up, so the wait a user is
/// told is never shorter than the real one.
fn human_wait(wait: Duration) -> String {
    let secs = wait.as_secs().max(1);
    if secs < 60 {
        let unit = if secs == 1 { "second" } else { "seconds" };
        return format!("{secs} {unit}");
    }
    let mins = secs.div_ceil(60);
    let unit = if mins == 1 { "minute" } else { "minutes" };
    format!("{mins} {unit}")
}

/// A 429 carrying `Retry-After` in whole seconds, and a body sentence that
/// states the wait for clients (the web UI) that only show the body.
pub(crate) fn throttled_response(retry_after: Duration) -> Response {
    let mut response = ApiError::status(
        StatusCode::TOO_MANY_REQUESTS,
        format!(
            "too many failed password attempts; try again in {}",
            human_wait(retry_after)
        ),
    )
    .into_response();
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        HeaderValue::from(retry_after.as_secs()),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len(buckets: &Buckets) -> usize {
        buckets.table.lock().unwrap().entries.len()
    }

    #[test]
    fn the_sixth_attempt_in_a_window_is_refused_with_the_remaining_wait() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for i in 0..MAX_PASSWORD_ATTEMPTS {
            assert!(throttle.begin("k", start).is_ok(), "attempt {i}");
        }
        let later = start + Duration::from_secs(60);
        let wait = throttle.begin("k", later).unwrap_err();
        assert_eq!(wait, THROTTLE_WINDOW - Duration::from_secs(60));
    }

    #[test]
    fn the_window_lapsing_restores_the_budget() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            throttle.begin("k", start).unwrap();
        }
        assert!(throttle.begin("k", start).is_err());
        assert!(throttle.begin("k", start + THROTTLE_WINDOW).is_ok());
    }

    #[test]
    fn a_success_clears_the_key_and_keys_are_independent() {
        let throttle = LoginThrottle::default();
        let now = Instant::now();
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            throttle.begin("a", now).unwrap();
        }
        assert!(throttle.begin("a", now).is_err());
        assert!(throttle.begin("b", now).is_ok());
        throttle.clear("a");
        assert!(throttle.begin("a", now).is_ok());
    }

    #[test]
    fn retry_after_rounds_up_and_is_never_zero() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            throttle.begin("k", start).unwrap();
        }
        let nearly = start + THROTTLE_WINDOW - Duration::from_millis(1);
        assert_eq!(
            throttle.begin("k", nearly).unwrap_err(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn the_map_is_bounded_and_sweeps_in_batches() {
        let buckets = Buckets::new(MAX_PASSWORD_ATTEMPTS, 100);
        let now = Instant::now();
        for i in 0..100 {
            buckets.begin(&format!("k{i}"), now).unwrap();
        }
        assert_eq!(len(&buckets), 100);
        // The 101st key sweeps once, freeing a whole batch, so the next
        // batch-minus-one new keys need no sweep at all.
        buckets.begin("k100", now).unwrap();
        assert_eq!(len(&buckets), 100 - buckets.sweep_batch() + 1);
        for i in 0..1_000 {
            buckets.begin(&format!("more{i}"), now).unwrap();
            assert!(len(&buckets) <= 100);
        }
    }

    #[test]
    fn lapsed_windows_are_swept_before_live_ones() {
        let buckets = Buckets::new(MAX_PASSWORD_ATTEMPTS, 10);
        let start = Instant::now();
        for i in 0..5 {
            buckets.begin(&format!("old{i}"), start).unwrap();
        }
        let later = start + THROTTLE_WINDOW;
        for i in 0..5 {
            buckets.begin(&format!("live{i}"), later).unwrap();
        }
        buckets.begin("new", later).unwrap();
        let table = buckets.table.lock().unwrap();
        assert_eq!(table.entries.len(), 6);
        assert!(table.entries.keys().all(|k| !k.starts_with("old")));
    }

    #[test]
    fn a_locked_bucket_survives_map_pressure() {
        let buckets = Buckets::new(MAX_PASSWORD_ATTEMPTS, 10);
        let now = Instant::now();
        // The target is locked first, so it is the oldest entry — the one
        // an evict-the-oldest policy would have dropped.
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            buckets.begin("target", now).unwrap();
        }
        // A partly-spent bucket, also older than the spray.
        for _ in 0..MAX_PASSWORD_ATTEMPTS - 1 {
            buckets.begin("partial", now).unwrap();
        }
        let spray = now + Duration::from_secs(1);
        for i in 0..1_000 {
            buckets.begin(&format!("spray{i}"), spray).unwrap();
        }
        assert!(buckets.begin("target", spray).is_err(), "lock survived");
        // The partial bucket kept its count: one attempt left, then locked.
        assert!(buckets.begin("partial", spray).is_ok());
        assert!(buckets.begin("partial", spray).is_err());
    }

    #[test]
    fn a_map_full_of_locks_fails_closed_for_new_keys() {
        let buckets = Buckets::new(MAX_PASSWORD_ATTEMPTS, 3);
        let start = Instant::now();
        for k in ["a", "b", "c"] {
            for _ in 0..MAX_PASSWORD_ATTEMPTS {
                buckets.begin(k, start).unwrap();
            }
        }
        let now = start + Duration::from_secs(60);
        let wait = buckets.begin("new", now).unwrap_err();
        assert_eq!(wait, THROTTLE_WINDOW - Duration::from_secs(60));
        // Nothing was evicted to make the refusal.
        assert_eq!(len(&buckets), 3);
        assert!(buckets.begin("a", now).is_err());
        // Refused again without a sweep, until a lock lapses...
        assert!(buckets.begin("new", now).is_err());
        assert!(buckets.table.lock().unwrap().full_until.is_some());
        assert!(buckets.begin("new", start + THROTTLE_WINDOW).is_ok());
        // ...or a key is cleared.
        let buckets = Buckets::new(MAX_PASSWORD_ATTEMPTS, 1);
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            buckets.begin("a", start).unwrap();
        }
        assert!(buckets.begin("new", start).is_err());
        buckets.clear("a");
        assert!(buckets.begin("new", start).is_ok());
    }

    #[test]
    fn the_key_ignores_the_port_and_degrades_to_username_without_a_peer() {
        let a: SocketAddr = "10.0.0.1:4000".parse().unwrap();
        let b: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        assert_eq!(
            login_throttle_key(Some(a), "root"),
            login_throttle_key(Some(b), "root")
        );
        assert_ne!(
            login_throttle_key(Some(a), "root"),
            login_throttle_key(None, "root")
        );
        assert_ne!(
            login_throttle_key(None, "root"),
            login_throttle_key(None, "rooT")
        );
    }

    #[test]
    fn the_key_buckets_ipv6_per_64_and_mapped_v4_as_v4() {
        let a: SocketAddr = "[2001:db8:1:2::1]:4000".parse().unwrap();
        let b: SocketAddr = "[2001:db8:1:2:ffff::9]:4000".parse().unwrap();
        let c: SocketAddr = "[2001:db8:1:3::1]:4000".parse().unwrap();
        assert_eq!(
            login_throttle_key(Some(a), "root"),
            login_throttle_key(Some(b), "root")
        );
        assert_ne!(
            login_throttle_key(Some(a), "root"),
            login_throttle_key(Some(c), "root")
        );
        let mapped: SocketAddr = "[::ffff:10.0.0.1]:4000".parse().unwrap();
        let v4: SocketAddr = "10.0.0.1:4000".parse().unwrap();
        assert_eq!(
            login_throttle_key(Some(mapped), "root"),
            login_throttle_key(Some(v4), "root")
        );
    }

    #[test]
    fn the_wait_is_stated_in_rounded_up_human_units() {
        assert_eq!(human_wait(Duration::from_secs(1)), "1 second");
        assert_eq!(human_wait(Duration::from_secs(45)), "45 seconds");
        assert_eq!(human_wait(Duration::from_secs(60)), "1 minute");
        assert_eq!(human_wait(Duration::from_secs(61)), "2 minutes");
        assert_eq!(human_wait(THROTTLE_WINDOW), "15 minutes");
    }
}
