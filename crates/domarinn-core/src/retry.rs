//! Retry policy and the shared backoff loop.
//!
//! Split out of `runner.rs` to keep one policy rather than growing several. It
//! is `pub` rather than `pub(crate)` because [`crate::DefaultGrader`] is public
//! API and is constructed with a policy by embedders.
//!
//! **Scope, precisely:** this governs the *provider* call and nothing else —
//! its one production caller is the runner's cached provider path. The module
//! header used to claim the LLM grader and the embeddings client shared it,
//! which was never true and sent readers looking for a `runner.retries` knob
//! that does not reach either.
//!
//! Grader calls do retry, but on their own terms and not through here: see
//! `request_cache::ask_live`. The two are deliberately different. A provider
//! failure is transport-shaped, so it backs off, honours `Retry-After` and
//! keys on [`ProviderError::Retriable`]. A grader failure is usually
//! *sampling*-shaped — the judge answered, just not usably — so re-asking
//! immediately is the point and a backoff would only slow a run down. The
//! embeddings client still has no retry at all.

use std::future::Future;
use std::time::{Duration, Instant};

use crate::config::Suite;
use crate::provider::ProviderError;

const DEFAULT_MAX_RETRIES: u32 = 2;
const DEFAULT_INITIAL_MS: u64 = 500;
const DEFAULT_MAX_MS: u64 = 8_000;
/// Ceiling on a server-supplied `Retry-After`. Deliberately *not* `max_ms`:
/// that bounds domarinn's own exponential guesswork, while `Retry-After` is a
/// directive from the server describing its rate window.
const DEFAULT_RETRY_AFTER_MAX_MS: u64 = 120_000;

/// Retry/backoff settings derived from the suite `runner.retries`.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max: u32,
    pub initial_ms: u64,
    pub max_ms: u64,
    /// Spread concurrent retries so rate-limited cells do not all wake at once.
    pub jitter: bool,
    /// Largest `Retry-After` to honor. A larger hint gives up instead of
    /// retrying early — see [`with_retry`].
    pub retry_after_max_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            // Retries are ON by default. A single 429 or 502 otherwise scores
            // the case 0, which reads as a model failure rather than a blip.
            // Safe only because backoff is excluded from `latency_ms` and
            // `Retry-After` is honored rather than clamped — without those two,
            // this default breaks every `latency` assert and hammers rate
            // limiters.
            max: DEFAULT_MAX_RETRIES,
            initial_ms: DEFAULT_INITIAL_MS,
            max_ms: DEFAULT_MAX_MS,
            jitter: false,
            retry_after_max_ms: DEFAULT_RETRY_AFTER_MAX_MS,
        }
    }
}

impl RetryPolicy {
    pub fn from_suite(suite: &Suite) -> Self {
        match suite.runner.as_ref().and_then(|r| r.retries.as_ref()) {
            Some(r) => RetryPolicy {
                max: r.max,
                initial_ms: r.initial_ms.unwrap_or(DEFAULT_INITIAL_MS),
                max_ms: r.max_ms.unwrap_or(DEFAULT_MAX_MS),
                jitter: r.jitter,
                retry_after_max_ms: r.retry_after_max_ms.unwrap_or(DEFAULT_RETRY_AFTER_MAX_MS),
            },
            None => RetryPolicy::default(),
        }
    }

    /// Backoff before attempt `attempt` (1-based), honoring a server hint.
    ///
    /// Deliberately pure: jitter is applied by [`with_retry`], which knows
    /// whether the delay came from a server directive or from our own
    /// exponential. Jittering a `Retry-After` would retry *earlier* than the
    /// server permitted — full jitter averages half the requested delay.
    pub fn backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(hint) = retry_after {
            return hint;
        }
        let exp = self
            .initial_ms
            .saturating_mul(1u64 << attempt.min(16).saturating_sub(1));
        Duration::from_millis(exp.min(self.max_ms))
    }
}

/// Full jitter over `(0, delay]`.
///
/// Seeded from the wall clock rather than a `rand` dependency: the goal is
/// decorrelating concurrent workers, not unpredictability.
fn jittered(delay: Duration) -> Duration {
    let ms = delay.as_millis() as u64;
    if ms == 0 {
        return delay;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_millis(1 + nanos % ms)
}

/// What a [`with_retry`] call cost, regardless of whether it succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryStats {
    /// Attempts actually made, 1-based. Reported on the failure path too: the
    /// runner previously hardcoded `1` for errored cases, so a case that burned
    /// three attempts claimed it had made one.
    pub attempts: u32,
    /// Time spent **inside** the operation, summed across attempts and
    /// excluding backoff sleeps.
    ///
    /// This is the number a `latency` assertion must see. Measuring around the
    /// whole retry loop instead would charge backoff to the provider, so a
    /// single 429 would fail `{type: latency, max: 2000}` on a model that
    /// answered in 30 ms.
    pub in_flight: Duration,
}

/// Run `op` until it succeeds, hits a fatal error, or exhausts the policy.
///
/// `op` receives the 1-based attempt number.
pub async fn with_retry<T, F, Fut>(
    policy: &RetryPolicy,
    mut op: F,
) -> (Result<T, ProviderError>, RetryStats)
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut attempts = 0u32;
    let mut in_flight = Duration::ZERO;
    loop {
        attempts += 1;
        let started = Instant::now();
        let outcome = op(attempts).await;
        in_flight += started.elapsed();
        let stats = RetryStats {
            attempts,
            in_flight,
        };

        match outcome {
            Ok(value) => return (Ok(value), stats),
            Err(ProviderError::Retriable {
                source,
                retry_after,
                class,
                // Threaded through both exits below, not dropped. This is the
                // one place a provider's structured diagnostics could vanish
                // precisely on the failure that gets reported — the retry
                // budget ran out, so this error is the one a human reads.
                details,
            }) => {
                if attempts > policy.max {
                    return (
                        Err(ProviderError::Retriable {
                            source,
                            retry_after,
                            class,
                            details,
                        }),
                        stats,
                    );
                }

                // A `Retry-After` beyond the ceiling means giving up, not
                // retrying early. Retrying before the server's window closes
                // just earns another 429 — and burns the remaining attempts
                // doing it.
                let ceiling = Duration::from_millis(policy.retry_after_max_ms);
                if let Some(hint) = retry_after {
                    if hint > ceiling {
                        return (
                            // Fatal now, but still a rate limit: the variant
                            // says whether retrying could help, the class says
                            // what went wrong. They are independent axes.
                            Err(ProviderError::Fatal {
                                source: anyhow::anyhow!(
                                    "server asked to retry after {}s, above the {}s ceiling \
                                     (runner.retries.retry_after_max_ms): {source}",
                                    hint.as_secs(),
                                    ceiling.as_secs()
                                ),
                                class,
                                details,
                            }),
                            stats,
                        );
                    }
                }

                // Honor a directive verbatim; jitter only our own guesswork.
                let delay = match retry_after {
                    Some(hint) => hint,
                    None => {
                        let base = policy.backoff(attempts, None);
                        if policy.jitter {
                            jittered(base)
                        } else {
                            base
                        }
                    }
                };
                tracing::warn!(
                    attempt = attempts,
                    delay_ms = delay.as_millis() as u64,
                    error = %source,
                    "retriable provider error; retrying"
                );
                tokio::time::sleep(delay).await;
            }
            Err(fatal) => return (Err(fatal), stats),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_class::ErrorClass;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn backoff_grows_and_caps() {
        let policy = RetryPolicy {
            max: 5,
            initial_ms: 100,
            max_ms: 1000,
            ..Default::default()
        };
        assert_eq!(policy.backoff(1, None), Duration::from_millis(100));
        assert_eq!(policy.backoff(2, None), Duration::from_millis(200));
        assert_eq!(policy.backoff(3, None), Duration::from_millis(400));
        // Caps at max_ms.
        assert_eq!(policy.backoff(10, None), Duration::from_millis(1000));
    }

    #[test]
    fn backoff_honors_retry_after_hint() {
        let policy = RetryPolicy {
            max: 5,
            initial_ms: 100,
            max_ms: 1000,
            ..Default::default()
        };
        assert_eq!(
            policy.backoff(1, Some(Duration::from_secs(7))),
            Duration::from_secs(7)
        );
    }

    #[tokio::test]
    async fn reports_attempts_on_the_failure_path() {
        // The whole reason `with_retry` returns the count rather than only the
        // value: an errored case must be able to say it burned three attempts.
        let policy = RetryPolicy {
            max: 2,
            initial_ms: 1,
            max_ms: 2,
            ..Default::default()
        };
        let (result, stats) = with_retry(&policy, |_| async {
            Err::<(), _>(ProviderError::retriable(
                ErrorClass::PROVIDER_UNAVAILABLE,
                anyhow::anyhow!("boom"),
                None,
            ))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(stats.attempts, 3, "one initial attempt plus two retries");
    }

    #[tokio::test]
    async fn a_fatal_error_is_not_retried() {
        let policy = RetryPolicy {
            max: 5,
            initial_ms: 1,
            max_ms: 2,
            ..Default::default()
        };
        let calls = AtomicU32::new(0);
        let (result, stats) = with_retry(&policy, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async {
                Err::<(), _>(ProviderError::fatal(
                    ErrorClass::PROVIDER_REQUEST,
                    anyhow::anyhow!("bad request"),
                ))
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(stats.attempts, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn succeeds_without_retrying_when_the_first_attempt_works() {
        let policy = RetryPolicy {
            max: 3,
            initial_ms: 1,
            max_ms: 2,
            ..Default::default()
        };
        let (result, stats) =
            with_retry(&policy, |n| async move { Ok::<_, ProviderError>(n) }).await;
        assert_eq!(result.unwrap(), 1);
        assert_eq!(stats.attempts, 1);
    }

    /// `max_ms` bounds *our* exponential guesswork; a server hint is not a
    /// guess. Clamping `Retry-After: 60` to an 8s ceiling guarantees another 429
    /// inside the same rate window and burns the remaining attempts doing it.
    #[tokio::test]
    async fn retry_after_is_honored_verbatim_not_clamped_to_max_ms() {
        let policy = RetryPolicy {
            max: 5,
            initial_ms: 100,
            max_ms: 8_000,
            ..Default::default()
        };
        assert_eq!(
            policy.backoff(1, Some(Duration::from_secs(60))),
            Duration::from_secs(60)
        );
    }

    /// Above the ceiling the case gives up. Retrying early would be worse than
    /// failing: it re-hammers a server that just said when to come back.
    #[tokio::test]
    async fn a_retry_after_beyond_the_ceiling_gives_up_rather_than_retrying_early() {
        let policy = RetryPolicy {
            max: 5,
            initial_ms: 1,
            max_ms: 2,
            retry_after_max_ms: 60_000,
            ..Default::default()
        };
        let (result, stats) = with_retry(&policy, |_| async {
            Err::<(), _>(ProviderError::retriable(
                ErrorClass::PROVIDER_UNAVAILABLE,
                anyhow::anyhow!("rate limited"),
                Some(Duration::from_secs(600)),
            ))
        })
        .await;

        assert!(matches!(result, Err(ProviderError::Fatal { .. })));
        assert_eq!(stats.attempts, 1, "no further attempt is made");
        let message = format!("{}", result.unwrap_err());
        assert!(
            message.contains("600s"),
            "names the requested delay: {message}"
        );
        assert!(message.contains("60s"), "names the ceiling: {message}");
    }

    /// Full jitter is `random(0, delay]`. Applied to a server directive it
    /// retries *earlier* than permitted — on average at half the requested wait.
    #[tokio::test]
    async fn jitter_never_applies_to_a_server_hint() {
        let policy = RetryPolicy {
            max: 5,
            initial_ms: 100,
            max_ms: 1000,
            jitter: true,
            ..Default::default()
        };
        for _ in 0..20 {
            assert_eq!(
                policy.backoff(1, Some(Duration::from_secs(30))),
                Duration::from_secs(30)
            );
        }
    }

    #[test]
    fn jitter_spreads_the_exponential_branch_within_bounds() {
        let base = Duration::from_millis(800);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let d = jittered(base);
            assert!(d > Duration::ZERO && d <= base, "out of bounds: {d:?}");
            seen.insert(d.as_millis());
        }
        assert!(seen.len() > 1, "jitter produced a single value");
    }
}
