//! A per-identity token bucket for `tools/call`.
//!
//! The spec makes rate limiting a MUST for tool invocations. The bucket
//! mechanics live in [`crate::ratelimit`]; this module only decides what a
//! caller's key is.
//!
//! **Keyed on the account, then the client address.** An account-backed
//! identity gets one bucket wherever it connects from: the account is a better
//! identity than an address, and a user moving between networks should not
//! mint fresh budgets. Anonymous callers and static-token holders have no
//! identity of their own (a static token's only identity is its scope label),
//! so they are told apart by the resolved [`ClientIp`] — one bucket per label
//! per client, rather than one bucket every holder of the token shares. Under
//! the `oneshot` test harness there is no `ClientIp`, and the keys fall back to
//! the label alone.

use std::time::{Duration, Instant};

use crate::auth::Identity;
use crate::clientip::ClientIp;
use crate::ratelimit::TokenBuckets;

/// Sustained `tools/call` rate per identity.
pub const DEFAULT_PER_MINUTE: u32 = 60;
/// How many calls may arrive back-to-back before the sustained rate applies.
pub const DEFAULT_BURST: u32 = 10;
/// Cap on tracked keys; past it the fullest buckets are evicted.
const MAX_TRACKED: usize = 4096;

/// The `tools/call` limiter: [`TokenBuckets`] keyed by [`bucket_key`].
pub struct RateLimiter {
    buckets: TokenBuckets,
}

impl RateLimiter {
    pub fn new(per_minute: u32, burst: u32) -> RateLimiter {
        RateLimiter {
            buckets: TokenBuckets::new(per_minute, burst, MAX_TRACKED),
        }
    }

    /// Take one token for `identity` calling from `client`.
    /// `Err(retry_after)` when the bucket is empty.
    pub fn check(&self, identity: &Identity, client: Option<&ClientIp>) -> Result<(), Duration> {
        self.buckets
            .take(&bucket_key(identity, client), Instant::now())
    }
}

impl Default for RateLimiter {
    fn default() -> RateLimiter {
        RateLimiter::new(DEFAULT_PER_MINUTE, DEFAULT_BURST)
    }
}

/// The bucket an identity falls into. Without a client address, anonymous
/// callers share one bucket: they are indistinguishable to us.
fn bucket_key(identity: &Identity, client: Option<&ClientIp>) -> String {
    if let Some(user_id) = &identity.user_id {
        return format!("user:{}", user_id.as_str());
    }
    let base = match &identity.label {
        Some(label) => format!("label:{label}"),
        None => "anonymous".to_string(),
    };
    match client {
        Some(client) => format!("{base}@{}", client.bucket()),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn client(addr: &str) -> ClientIp {
        ClientIp(addr.parse::<IpAddr>().unwrap())
    }

    fn anonymous() -> Identity {
        Identity::anonymous(crate::AuthMode::Closed)
    }

    #[test]
    fn anonymous_callers_from_different_addresses_get_different_buckets() {
        let a = bucket_key(&anonymous(), Some(&client("192.0.2.10")));
        let b = bucket_key(&anonymous(), Some(&client("198.51.100.20")));
        assert_eq!(a, "anonymous@192.0.2.10");
        assert_eq!(b, "anonymous@198.51.100.20");
    }

    #[test]
    fn static_token_holders_are_split_by_address() {
        let mut identity = anonymous();
        identity.label = Some("ci".to_string());
        assert_eq!(
            bucket_key(&identity, Some(&client("203.0.113.5"))),
            "label:ci@203.0.113.5"
        );
        // IPv6 callers fall into their /64, as `ClientIp::bucket` defines.
        assert_eq!(
            bucket_key(&identity, Some(&client("2001:db8:1:2::abcd"))),
            format!("label:ci@{}", client("2001:db8:1:2::abcd").bucket())
        );
    }

    #[test]
    fn a_user_backed_identity_ignores_the_client_address() {
        let mut identity = anonymous();
        identity.user_id = Some(crate::domain::UserId::new("u1"));
        identity.label = Some("root".to_string());
        // The account id wins over both the display label and the address.
        assert_eq!(
            bucket_key(&identity, Some(&client("192.0.2.10"))),
            "user:u1"
        );
        assert_eq!(
            bucket_key(&identity, Some(&client("198.51.100.20"))),
            "user:u1"
        );
        assert_eq!(bucket_key(&identity, None), "user:u1");
    }

    #[test]
    fn without_a_client_address_the_keys_are_the_legacy_ones() {
        assert_eq!(bucket_key(&anonymous(), None), "anonymous");
        let mut identity = anonymous();
        identity.label = Some("ci".to_string());
        assert_eq!(bucket_key(&identity, None), "label:ci");
    }

    #[test]
    fn the_limit_applies_per_client_address() {
        let limiter = RateLimiter::new(60, 1);
        let anon = anonymous();
        let first = client("192.0.2.10");
        assert!(limiter.check(&anon, Some(&first)).is_ok());
        assert!(limiter.check(&anon, Some(&first)).is_err());
        assert!(limiter.check(&anon, Some(&client("192.0.2.11"))).is_ok());
    }
}
