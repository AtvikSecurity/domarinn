//! Device cookies: a signed note that "this browser has signed in as this user
//! before" (the OWASP "Slow Down Online Guessing Attacks with Device Cookies"
//! scheme).
//!
//! A recognised browser's failed sign-ins are counted per device instead of
//! per account, so an attacker hammering an account cannot lock its owner out
//! of a browser they already use. The cookie authenticates nothing; it only
//! decides which failure counter an attempt is charged to.

use anyhow::{bail, Context};
use axum::http::{HeaderMap, HeaderValue};
use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use sha2::Sha256;

use crate::domain::UserId;

/// Name of the device cookie.
pub(crate) const DEVICE_COOKIE: &str = "domarinn_device";
/// One year: the cookie should outlive any lockout an attacker can cause.
const DEVICE_COOKIE_MAX_AGE_SECS: i64 = 365 * 24 * 60 * 60;
/// Format tag, so a later change to the layout or MAC can reject old cookies
/// outright instead of misreading them.
const VERSION: &str = "v1";
/// Random bytes in a nonce (hex-encoded to twice this).
const NONCE_BYTES: usize = 16;

/// A decoded, MAC-verified device cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceCookie {
    pub(crate) user_id: UserId,
    /// Per-browser identity; the trusted path keys its counter on it.
    pub(crate) nonce: String,
}

/// The server secret device cookies are signed with. It is shared through the
/// database so every replica accepts every other replica's cookies.
pub(crate) struct DeviceKey([u8; 32]);

impl DeviceKey {
    /// Parse the stored secret: 64 hex chars. Anything else is a corrupt row,
    /// not something to quietly sign with.
    pub(crate) fn from_hex(hex: &str) -> anyhow::Result<Self> {
        let bytes = hex::decode(hex).context("device cookie key is not hex")?;
        let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) else {
            bail!("device cookie key must be 32 bytes, got {}", bytes.len());
        };
        Ok(DeviceKey(key))
    }

    /// A cookie for a browser that has just signed in as `user_id`.
    pub(crate) fn fresh(&self, user_id: &UserId) -> DeviceCookie {
        let mut nonce = [0u8; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce);
        DeviceCookie {
            user_id: user_id.clone(),
            nonce: hex::encode(nonce),
        }
    }

    /// `v1.<user_id>.<nonce>.<mac>`, the MAC covering everything before it.
    pub(crate) fn encode(&self, cookie: &DeviceCookie) -> String {
        let payload = format!("{VERSION}.{}.{}", cookie.user_id, cookie.nonce);
        let mac = hex::encode(self.mac(&payload).finalize().into_bytes());
        format!("{payload}.{mac}")
    }

    /// The cookie `value` carries, if this key signed it and it is well formed.
    pub(crate) fn decode(&self, value: &str) -> Option<DeviceCookie> {
        let mut parts = value.split('.');
        let (Some(version), Some(user_id), Some(nonce), Some(mac), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return None;
        };
        if version != VERSION
            || user_id.is_empty()
            || nonce.len() != NONCE_BYTES * 2
            || !nonce.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        let mac = hex::decode(mac).ok()?;
        // `verify_slice` compares in constant time, so response timing does not
        // leak how much of a forged MAC was right.
        self.mac(&format!("{version}.{user_id}.{nonce}"))
            .verify_slice(&mac)
            .ok()?;
        Some(DeviceCookie {
            user_id: UserId::new(user_id),
            nonce: nonce.to_string(),
        })
    }

    fn mac(&self, payload: &str) -> Hmac<Sha256> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC accepts keys of any length");
        mac.update(payload.as_bytes());
        mac
    }
}

/// The browser's device cookie for `user_id`, if it has one. A valid cookie
/// for a different user counts as absent: it says nothing about this account.
/// Every `Cookie` header and every `domarinn_device` in them is tried, so a
/// stray or stale duplicate cannot mask the real one.
pub(crate) fn recognised(
    headers: &HeaderMap,
    key: &DeviceKey,
    user_id: Option<&UserId>,
) -> Option<DeviceCookie> {
    let user_id = user_id?;
    for value in headers.get_all(axum::http::header::COOKIE) {
        let Ok(raw) = value.to_str() else { continue };
        for parsed in cookie::Cookie::split_parse(raw).flatten() {
            if parsed.name() != DEVICE_COOKIE {
                continue;
            }
            if let Some(device) = key.decode(parsed.value()) {
                if &device.user_id == user_id {
                    return Some(device);
                }
            }
        }
    }
    None
}

/// `Set-Cookie` value installing the device cookie. `HttpOnly` keeps it from
/// script; `SameSite=Strict` and `Path=/api/v1/auth` mean it only travels on
/// same-site requests to the auth endpoints, the only place it is read;
/// `Secure` follows the session cookie's rule.
pub(crate) fn set_cookie(value: &str, secure: bool) -> HeaderValue {
    let cookie = cookie::Cookie::build((DEVICE_COOKIE, value))
        .http_only(true)
        .same_site(cookie::SameSite::Strict)
        .path("/api/v1/auth")
        .max_age(cookie::time::Duration::seconds(DEVICE_COOKIE_MAX_AGE_SECS))
        .secure(secure)
        .build();
    HeaderValue::from_str(&cookie.to_string()).expect("device cookie is always valid ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    const KEY_B: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

    fn key() -> DeviceKey {
        DeviceKey::from_hex(KEY_A).unwrap()
    }

    fn user(id: &str) -> UserId {
        UserId::new(id)
    }

    fn headers_with(cookie_headers: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in cookie_headers {
            headers.append(
                axum::http::header::COOKIE,
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    /// Replace the char at `index` with a different hex digit.
    fn flip(value: &str, index: usize) -> String {
        let mut chars: Vec<char> = value.chars().collect();
        chars[index] = if chars[index] == '0' { '1' } else { '0' };
        chars.into_iter().collect()
    }

    #[test]
    fn a_cookie_round_trips() {
        let key = key();
        let cookie = key.fresh(&user("01JUSERA"));
        assert_eq!(cookie.nonce.len(), 32);
        assert!(cookie.nonce.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(key.decode(&key.encode(&cookie)), Some(cookie));
    }

    #[test]
    fn fresh_nonces_differ() {
        let key = key();
        let u = user("01JUSERA");
        assert_ne!(key.fresh(&u).nonce, key.fresh(&u).nonce);
    }

    #[test]
    fn the_encoding_has_the_documented_shape() {
        let key = key();
        let encoded = key.encode(&key.fresh(&user("01JUSERA")));
        let parts: Vec<&str> = encoded.split('.').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "v1");
        assert_eq!(parts[1], "01JUSERA");
        assert_eq!(parts[2].len(), 32);
        assert_eq!(parts[3].len(), 64);
        assert!(parts[3].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tampering_is_rejected() {
        let key = key();
        let good = key.encode(&key.fresh(&user("01JUSERA")));
        assert!(key.decode(&good).is_some());

        // Offsets: "v1." is 3 chars, user id 8, then ".", nonce 32, ".", MAC 64.
        let user_at = 3;
        let nonce_at = 3 + 8 + 1;
        let mac_at = nonce_at + 32 + 1;
        let other_key = DeviceKey::from_hex(KEY_B).unwrap();
        let cases = [
            flip(&good, user_at),
            flip(&good, nonce_at),
            flip(&good, mac_at),
            flip(&good, good.len() - 1),
            good.replacen("v1.", "v2.", 1),
            format!("{good}.x"),
            good[..good.len() - 2].to_string(),
            String::new(),
            other_key.encode(&other_key.fresh(&user("01JUSERA"))),
        ];
        for case in cases {
            assert_eq!(key.decode(&case), None, "accepted {case:?}");
        }
    }

    #[test]
    fn an_empty_user_or_malformed_nonce_is_rejected_even_when_signed() {
        let key = key();
        let empty_user = DeviceCookie {
            user_id: user(""),
            nonce: "0".repeat(32),
        };
        assert_eq!(key.decode(&key.encode(&empty_user)), None);
        let short_nonce = DeviceCookie {
            user_id: user("01JUSERA"),
            nonce: "0".repeat(31),
        };
        assert_eq!(key.decode(&key.encode(&short_nonce)), None);
        let non_hex_nonce = DeviceCookie {
            user_id: user("01JUSERA"),
            nonce: "z".repeat(32),
        };
        assert_eq!(key.decode(&key.encode(&non_hex_nonce)), None);
    }

    #[test]
    fn a_cookie_for_another_user_is_absent() {
        let key = key();
        let (a, b) = (user("01JUSERA"), user("01JUSERB"));
        let encoded = key.encode(&key.fresh(&a));
        let headers = headers_with(&[&format!("{DEVICE_COOKIE}={encoded}")]);
        assert_eq!(recognised(&headers, &key, Some(&b)), None);
        assert_eq!(
            recognised(&headers, &key, Some(&a)).map(|c| c.user_id),
            Some(a)
        );
        assert_eq!(recognised(&headers, &key, None), None);
    }

    #[test]
    fn the_first_matching_cookie_wins_across_headers() {
        let key = key();
        let a = user("01JUSERA");
        let cookie = key.fresh(&a);
        let encoded = key.encode(&cookie);
        let headers = headers_with(&[
            &format!("other=1; {DEVICE_COOKIE}=garbage"),
            &format!("{DEVICE_COOKIE}={encoded}"),
        ]);
        assert_eq!(recognised(&headers, &key, Some(&a)), Some(cookie));
    }

    #[test]
    fn the_set_cookie_carries_every_attribute() {
        for secure in [false, true] {
            let header = set_cookie("v1.u.n.m", secure);
            let s = header.to_str().unwrap();
            assert!(s.starts_with(&format!("{DEVICE_COOKIE}=v1.u.n.m")), "{s}");
            assert!(s.contains("HttpOnly"), "{s}");
            assert!(s.contains("SameSite=Strict"), "{s}");
            assert!(s.contains("Path=/api/v1/auth"), "{s}");
            assert!(s.contains("Max-Age=31536000"), "{s}");
            assert_eq!(s.contains("Secure"), secure, "{s}");
        }
    }

    #[test]
    fn from_hex_rejects_short_or_non_hex_keys() {
        assert!(DeviceKey::from_hex(KEY_A).is_ok());
        assert!(DeviceKey::from_hex("").is_err());
        assert!(DeviceKey::from_hex(&KEY_A[..62]).is_err());
        assert!(DeviceKey::from_hex(&format!("{KEY_A}00")).is_err());
        assert!(DeviceKey::from_hex(&"zz".repeat(32)).is_err());
    }
}
