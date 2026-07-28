//! Shared helpers for HTTP-backed providers.
//!
//! Central place for the retry/timeout classification so every network provider
//! treats 429/5xx/timeouts as retriable (honoring `Retry-After`) and 4xx as
//! fatal, consistently.

use std::time::Duration;

use crate::error_class::ErrorClass;
use crate::provider::ProviderError;

/// A shared reqwest client with sane timeouts, built once per provider.
pub fn http_client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .unwrap_or_default()
}

/// Turn a transport-level reqwest error into a provider error (retriable when it
/// is a timeout or connection problem).
pub fn transport_error(e: reqwest::Error) -> ProviderError {
    if e.is_timeout() || e.is_connect() || e.is_request() {
        ProviderError::retriable(ErrorClass::PROVIDER_TIMEOUT, anyhow::Error::new(e), None)
    } else {
        // A transport error that is neither a timeout nor a connection failure
        // is a malformed exchange, not an unavailable server.
        ProviderError::fatal(ErrorClass::PROVIDER_PROTOCOL, anyhow::Error::new(e))
    }
}

/// Classify a non-success HTTP response into a provider error.
///
/// 429 and 5xx are retriable (with any `Retry-After`); other statuses are fatal.
pub fn status_error(
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
    body: String,
) -> ProviderError {
    let msg = format!("HTTP {}: {}", status.as_u16(), truncate(&body, 500));
    // Classified here because this is the last place the status code exists as
    // a number; below, it survives only inside `msg`.
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        ProviderError::retriable(
            ErrorClass::PROVIDER_RATE_LIMIT,
            anyhow::anyhow!(msg),
            retry_after,
        )
    } else if status.is_server_error() {
        ProviderError::retriable(
            ErrorClass::PROVIDER_UNAVAILABLE,
            anyhow::anyhow!(msg),
            retry_after,
        )
    } else if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
        ProviderError::fatal(ErrorClass::PROVIDER_AUTH, anyhow::anyhow!(msg))
    } else {
        ProviderError::fatal(ErrorClass::PROVIDER_REQUEST, anyhow::anyhow!(msg))
    }
}

/// Parse a `Retry-After` header.
///
/// RFC 9110 §10.2.3 permits two forms and real providers use both: delta-seconds
/// (`120`) and an HTTP-date (`Wed, 21 Oct 2015 07:28:00 GMT`). Handling only the
/// first silently yields `None` for the second, which reads as "no hint" and
/// drops the caller back to blind exponential backoff against a server that just
/// told it exactly when to return.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();

    if let Ok(secs) = raw.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }

    // HTTP-date form: the delay is the remainder from now. A date already in
    // the past means "retry immediately", not a negative wait.
    let when = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    let delta = when.timestamp() - chrono::Utc::now().timestamp();
    Some(Duration::from_secs(delta.max(0) as u64))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Read an API key from the named environment variable, or a fatal error naming
/// the missing variable.
pub fn api_key(env_name: &str) -> Result<String, ProviderError> {
    std::env::var(env_name).map_err(|_| {
        ProviderError::fatal(
            ErrorClass::PROVIDER_AUTH,
            anyhow::anyhow!("API key environment variable '{env_name}' is not set"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

    fn headers_with(retry_after: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_str(retry_after).unwrap());
        h
    }

    #[test]
    fn parses_the_delta_seconds_form() {
        assert_eq!(
            parse_retry_after(&headers_with("120")),
            Some(Duration::from_secs(120))
        );
    }

    /// RFC 9110 §10.2.3 permits an HTTP-date, and providers use it. Returning
    /// `None` here reads as "no hint" and drops the caller back to blind
    /// exponential backoff against a server that just said when to return.
    #[test]
    fn parses_the_http_date_form() {
        let when = chrono::Utc::now() + chrono::Duration::seconds(90);
        let header = when.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

        let parsed = parse_retry_after(&headers_with(&header)).expect("an HTTP-date is a hint");
        // Allow a second of slack for the clock ticking between the two calls.
        assert!(
            parsed.as_secs() >= 88 && parsed.as_secs() <= 90,
            "expected ~90s, got {}s",
            parsed.as_secs()
        );
    }

    /// A date already in the past means "retry now", not a negative wait that
    /// would underflow into an enormous delay.
    #[test]
    fn an_http_date_in_the_past_is_zero_not_negative() {
        let when = chrono::Utc::now() - chrono::Duration::seconds(300);
        let header = when.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        assert_eq!(
            parse_retry_after(&headers_with(&header)),
            Some(Duration::from_secs(0))
        );
    }

    #[test]
    fn an_unparseable_value_is_no_hint() {
        assert_eq!(parse_retry_after(&headers_with("soon-ish")), None);
    }
}
