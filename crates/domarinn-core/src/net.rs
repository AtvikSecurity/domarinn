//! Shared helpers for HTTP-backed providers.
//!
//! Central place for the retry/timeout classification so every network provider
//! treats 429/5xx/timeouts as retriable (honoring `Retry-After`) and 4xx as
//! fatal, consistently.

use std::time::Duration;

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
        ProviderError::Retriable {
            source: anyhow::Error::new(e),
            retry_after: None,
        }
    } else {
        ProviderError::Fatal(anyhow::Error::new(e))
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
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        ProviderError::Retriable {
            source: anyhow::anyhow!(msg),
            retry_after,
        }
    } else {
        ProviderError::Fatal(anyhow::anyhow!(msg))
    }
}

/// Parse a `Retry-After` header (delta-seconds form).
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?;
    let secs: u64 = value.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
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
        ProviderError::Fatal(anyhow::anyhow!(
            "API key environment variable '{env_name}' is not set"
        ))
    })
}
