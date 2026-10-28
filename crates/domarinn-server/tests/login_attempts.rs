//! Database-backed login throttling: the per-key attempt counter, its lock
//! ladder, pruning, and the server-secret store the device cookies key off.
//!
//! Every test goes through `Storage`, so the file runs on whichever backend
//! `DOMARINN_TEST_BACKEND` selects. Time is passed in explicitly (`now_ms`),
//! never read from the clock, so the windows can be stepped across exactly.

mod common;

use std::collections::HashSet;

use domarinn_server::storage::{
    lock_duration_ms, AttemptPolicy, AttemptVerdict, Storage, ATTEMPT_WINDOW_MS,
    LOCKOUT_ESCALATION_MS,
};
use domarinn_server::{AuthMode, Settings};

const MIN: i64 = 60 * 1000;
const T0: i64 = 1_700_000_000_000;
const P: AttemptPolicy = AttemptPolicy {
    max_failures: 10,
    lock_base_ms: 5 * MIN,
    lock_cap_ms: 60 * MIN,
};

async fn storage() -> (Storage, tempfile::TempDir) {
    let (_app, storage, dir) =
        common::test_app_with_storage(Settings::default(), AuthMode::Closed).await;
    (storage, dir)
}

async fn begin(s: &Storage, key: &str, now: i64) -> AttemptVerdict {
    s.begin_login_attempt(key.to_string(), now, P)
        .await
        .expect("begin_login_attempt")
}

/// Spend `n` attempts at `now`, asserting every one is allowed.
async fn spend(s: &Storage, key: &str, now: i64, n: usize) {
    for i in 0..n {
        assert_eq!(
            begin(s, key, now).await,
            AttemptVerdict::Allowed,
            "attempt {} of {n} on {key}",
            i + 1
        );
    }
}

fn locked(retry_after_ms: i64) -> AttemptVerdict {
    AttemptVerdict::Locked { retry_after_ms }
}

#[tokio::test]
async fn ten_attempts_are_allowed_and_the_eleventh_locks_for_five_minutes() {
    let (s, _dir) = storage().await;
    spend(&s, "acct:alice", T0, 10).await;
    assert_eq!(begin(&s, "acct:alice", T0).await, locked(5 * MIN));
    // A locked attempt reserves nothing and reports the remaining time.
    assert_eq!(begin(&s, "acct:alice", T0 + 4 * MIN).await, locked(MIN));
}

#[tokio::test]
async fn locks_double_within_a_day_and_cap_at_an_hour() {
    let (s, _dir) = storage().await;
    let mut now = T0;
    let mut durations = Vec::new();
    for _ in 0..6 {
        spend(&s, "acct:bob", now, 10).await;
        let AttemptVerdict::Locked { retry_after_ms } = begin(&s, "acct:bob", now).await else {
            panic!("the 11th attempt must lock");
        };
        durations.push(retry_after_ms / MIN);
        now += retry_after_ms;
    }
    assert_eq!(durations, [5, 10, 20, 40, 60, 60]);
}

/// The quiet day is measured from the *last* lock, not the first: a second
/// lock 20 hours in keeps the chain alive until a day after it.
#[tokio::test]
async fn the_escalation_resets_after_a_quiet_day() {
    let (s, _dir) = storage().await;
    spend(&s, "acct:carol", T0, 10).await;
    assert_eq!(begin(&s, "acct:carol", T0).await, locked(5 * MIN));

    let second = T0 + 20 * 60 * MIN;
    spend(&s, "acct:carol", second, 10).await;
    assert_eq!(begin(&s, "acct:carol", second).await, locked(10 * MIN));

    let quiet = second + LOCKOUT_ESCALATION_MS;
    spend(&s, "acct:carol", quiet, 10).await;
    assert_eq!(begin(&s, "acct:carol", quiet).await, locked(5 * MIN));
}

/// The chain slides with the attack: an account that keeps tripping the lock
/// stays at the cap past the first lock's 24 hours, rather than being handed
/// a 5-minute lock once a day.
#[tokio::test]
async fn a_chain_sustained_past_a_day_stays_at_the_cap() {
    let (s, _dir) = storage().await;
    let mut now = T0;
    let mut durations = Vec::new();
    while now <= T0 + LOCKOUT_ESCALATION_MS + 120 * MIN {
        spend(&s, "acct:erin", now, 10).await;
        let AttemptVerdict::Locked { retry_after_ms } = begin(&s, "acct:erin", now).await else {
            panic!("the 11th attempt must lock");
        };
        durations.push(retry_after_ms / MIN);
        now += retry_after_ms;
    }
    assert_eq!(durations[..5], [5, 10, 20, 40, 60]);
    assert!(
        durations[4..].iter().all(|&d| d == 60),
        "every lock after the ladder tops out must stay at the cap: {durations:?}"
    );
    assert!(durations.len() > 25, "the chain spanned more than a day");
}

#[tokio::test]
async fn a_lapsed_window_restores_the_budget() {
    let (s, _dir) = storage().await;
    spend(&s, "acct:dave", T0, 10).await;
    let later = T0 + ATTEMPT_WINDOW_MS;
    spend(&s, "acct:dave", later, 10).await;
    assert_eq!(begin(&s, "acct:dave", later).await, locked(5 * MIN));
}

#[tokio::test]
async fn clearing_restores_the_budget_and_keys_are_independent() {
    let (s, _dir) = storage().await;
    spend(&s, "a", T0, 10).await;
    assert_eq!(begin(&s, "b", T0).await, AttemptVerdict::Allowed);
    s.clear_login_attempts("a".to_string()).await.unwrap();
    spend(&s, "a", T0, 10).await;
    assert_eq!(begin(&s, "a", T0).await, locked(5 * MIN));
}

#[test]
fn lock_duration_ms_matches_the_ladder() {
    let ladder: Vec<i64> = (1..=7).map(|n| lock_duration_ms(n, P) / MIN).collect();
    assert_eq!(ladder, [5, 10, 20, 40, 60, 60, 60]);
    // A long-running attacker must not overflow the shift into a short (or
    // negative) lock.
    assert_eq!(lock_duration_ms(1000, P), 60 * MIN);
}

#[tokio::test]
async fn prune_drops_only_rows_whose_windows_and_locks_have_lapsed() {
    let (s, _dir) = storage().await;
    // A row with a live attempt window.
    spend(&s, "fresh", T0, 1).await;
    // A row that is locked right now.
    spend(&s, "locked", T0, 10).await;
    assert_eq!(begin(&s, "locked", T0).await, locked(5 * MIN));
    // A row locked two hours ago: its lock and window have lapsed, but the
    // escalation chain is still live and must survive so the next lock
    // doubles instead of restarting at the base.
    let earlier = T0 - 120 * MIN;
    spend(&s, "escalating", earlier, 10).await;
    assert_eq!(begin(&s, "escalating", earlier).await, locked(5 * MIN));

    assert_eq!(s.prune_login_attempts(T0 + 1).await.unwrap(), 0);
    // The fresh row's window has lapsed and it never locked.
    assert_eq!(
        s.prune_login_attempts(T0 + ATTEMPT_WINDOW_MS)
            .await
            .unwrap(),
        1
    );
    // A day past both chains, every row is dead weight.
    assert_eq!(
        s.prune_login_attempts(T0 + LOCKOUT_ESCALATION_MS + 60 * MIN)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        s.prune_login_attempts(T0 + 2 * LOCKOUT_ESCALATION_MS)
            .await
            .unwrap(),
        0
    );
}

/// The race this guards is N guesses all reading "9 failures so far" and all
/// being let through. One `Storage` handle cannot show it: `Db::write`
/// serialises every closure behind that handle's writer mutex, so even a
/// non-atomic read-then-write would pass. The attempts are therefore spread
/// over four handles (four writers, as four replicas would have), leaving the
/// database itself (SQLite's file lock, Postgres's row lock) as the only thing
/// serialising them.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_begins_never_exceed_the_budget() {
    let (handles, _dir) = common::test_storage_handles(4).await;
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(20));
    let tasks: Vec<_> = (0..20)
        .map(|i| {
            let s = handles[i % handles.len()].clone();
            let start = start.clone();
            tokio::spawn(async move {
                start.wait().await;
                begin(&s, "acct:race", T0).await
            })
        })
        .collect();
    let mut allowed = 0;
    for t in tasks {
        if t.await.unwrap() == AttemptVerdict::Allowed {
            allowed += 1;
        }
    }
    assert_eq!(allowed, 10);
}

#[tokio::test]
async fn two_storage_handles_share_counters() {
    let (a, b, _dir) = common::test_storage_pair().await;
    spend(&a, "acct:shared", T0, 5).await;
    spend(&b, "acct:shared", T0, 5).await;
    assert_eq!(begin(&a, "acct:shared", T0).await, locked(5 * MIN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_secret_is_stable_and_concurrent_initialisation_agrees() {
    let (a, b, _dir) = common::test_storage_pair().await;
    let tasks: Vec<_> = (0..8)
        .map(|i| {
            let s = if i % 2 == 0 { a.clone() } else { b.clone() };
            tokio::spawn(async move { s.server_secret("device_cookie_key".to_string()).await })
        })
        .collect();
    let mut values = HashSet::new();
    for t in tasks {
        values.insert(t.await.unwrap().expect("server_secret"));
    }
    assert_eq!(values.len(), 1, "every caller must see the same secret");
    let secret = values.into_iter().next().unwrap();
    assert_eq!(secret.len(), 64);
    assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));

    // Stable on a later read, and independent per name.
    assert_eq!(
        a.server_secret("device_cookie_key".to_string())
            .await
            .unwrap(),
        secret
    );
    let other = b.server_secret("other_key".to_string()).await.unwrap();
    assert_eq!(other.len(), 64);
    assert_ne!(other, secret);
}
