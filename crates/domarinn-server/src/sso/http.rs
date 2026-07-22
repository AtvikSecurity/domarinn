//! reqwest-backed implementation of openidconnect's `AsyncHttpClient`.
//!
//! openidconnect's own `reqwest` feature would pin a second reqwest version
//! into the lockfile; this ~60-line adapter reuses the workspace's reqwest
//! (same version/features as the CLI and cache crates) instead.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use openidconnect::{AsyncHttpClient, HttpRequest, HttpResponse};

/// Error from an IdP HTTP round trip.
#[derive(Debug)]
pub struct HttpClientError(anyhow::Error);

impl std::fmt::Display for HttpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for HttpClientError {}

/// The HTTP client used for OIDC discovery, JWKS, and token exchange (and
/// SAML metadata fetch). Never follows redirects — every IdP endpoint must
/// answer directly, and following redirects from operator-supplied URLs is
/// an SSRF vector.
#[derive(Debug, Clone)]
pub struct HttpClient(reqwest::Client);

impl HttpClient {
    pub fn new() -> anyhow::Result<HttpClient> {
        Ok(HttpClient(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
        ))
    }

    /// Plain GET returning the body as text (SAML metadata fetch).
    pub async fn get_text(&self, url: &str) -> anyhow::Result<String> {
        let response = self.0.get(url).send().await?.error_for_status()?;
        Ok(response.text().await?)
    }
}

impl<'c> AsyncHttpClient<'c> for HttpClient {
    type Error = HttpClientError;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, HttpClientError>> + Send + 'c>>;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        let client = self.0.clone();
        Box::pin(async move {
            let (parts, body) = request.into_parts();
            let mut builder = client.request(parts.method, parts.uri.to_string());
            for (name, value) in &parts.headers {
                builder = builder.header(name, value);
            }
            let response = builder.body(body).send().await.map_err(|e| {
                HttpClientError(anyhow::Error::new(e).context("IdP request failed"))
            })?;

            let status = response.status();
            let headers = response.headers().clone();
            let body = response
                .bytes()
                .await
                .map_err(|e| {
                    HttpClientError(anyhow::Error::new(e).context("reading IdP response"))
                })?
                .to_vec();

            let mut out = openidconnect::http::Response::builder().status(status);
            for (name, value) in &headers {
                out = out.header(name, value);
            }
            out.body(body)
                .map_err(|e| HttpClientError(anyhow::Error::new(e).context("building response")))
        })
    }
}
