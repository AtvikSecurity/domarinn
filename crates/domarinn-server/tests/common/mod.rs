//! Shared fixtures and request helpers for the integration tests.
#![allow(dead_code)]

pub mod mock_oidc;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use chrono::{DateTime, TimeZone, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use domarinn_core::asserts::AssertName;
use domarinn_core::ids::RunId;
use domarinn_core::result::{
    AssertResult, AssertStatus, CaseResult, CaseStatus, CellKey, CiMeta, FilterSpec, GitMeta,
    RunResult, RunSummary,
};
use domarinn_core::types::{Output, TokenUsage};
use domarinn_server::storage::CaseListFilter;
use domarinn_server::{build_app, ServerConfig, Settings};

/// Build a router backed by a fresh temp data dir. Returns the router and the
/// `TempDir` guard (keep it alive for the duration of the test). Most suites
/// exercise data endpoints, not auth, so this runs in open mode; auth-focused
/// tests use [`test_app_with_mode`] or set `Settings::auth_mode`.
pub async fn test_app(settings: Settings) -> (Router, TempDir) {
    test_app_with_mode(settings, domarinn_server::AuthMode::Open).await
}

/// [`test_app`] with an explicit configured [`domarinn_server::AuthMode`].
pub async fn test_app_with_mode(
    settings: Settings,
    auth_mode: domarinn_server::AuthMode,
) -> (Router, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let config = ServerConfig {
        port: 0,
        data_dir: dir.path().to_path_buf(),
        auth_mode,
    };
    let (app, _state) = build_app(&config, settings).await.expect("build_app");
    (app, dir)
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

pub struct Reply {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// All `Set-Cookie` header values on the response.
    pub fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect()
    }
}

async fn read_reply(resp: Response<Body>) -> Reply {
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

pub async fn get(app: &Router, uri: &str) -> Reply {
    send(app, "GET", uri, None, None, Vec::new()).await
}

pub async fn get_auth(app: &Router, uri: &str, token: Option<&str>) -> Reply {
    send(app, "GET", uri, token, None, Vec::new()).await
}

pub async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    content_encoding: Option<&str>,
    body: Vec<u8>,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(enc) = content_encoding {
        builder = builder.header("content-encoding", enc);
    }
    if !body.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let req = builder.body(Body::from(body)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    read_reply(resp).await
}

/// Like [`send`], but with arbitrary extra headers (cookie/CSRF/SSO tests).
/// A JSON `Content-Type` is added whenever a body is present, unless the
/// caller supplied an explicit one (e.g. form-encoded SAML posts).
pub async fn send_with_headers(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let has_content_type = headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
    if !body.is_empty() && !has_content_type {
        builder = builder.header("content-type", "application/json");
    }
    let req = builder.body(Body::from(body)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    read_reply(resp).await
}

/// POST a raw body with an explicit `Content-Type` (or none at all when
/// `content_type` is `None`). Unlike [`send`], this never adds a default
/// `Content-Type`, so it can exercise the missing-content-type (415) path.
pub async fn post_raw(app: &Router, uri: &str, content_type: Option<&str>, body: Vec<u8>) -> Reply {
    let mut builder = Request::builder().method("POST").uri(uri);
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let req = builder.body(Body::from(body)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    read_reply(resp).await
}

pub async fn post_json(app: &Router, uri: &str, token: Option<&str>, value: &Value) -> Reply {
    send(
        app,
        "POST",
        uri,
        token,
        None,
        serde_json::to_vec(value).unwrap(),
    )
    .await
}

pub async fn put_bytes(app: &Router, uri: &str, token: Option<&str>, body: Vec<u8>) -> Reply {
    send(app, "PUT", uri, token, None, body).await
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

// ---------------------------------------------------------------------------
// RunResult fixtures
// ---------------------------------------------------------------------------

/// Spec for a single case in a fixture run.
pub struct CaseSpec {
    pub provider: &'static str,
    pub prompt: Option<&'static str>,
    pub test: &'static str,
    pub repeat: u32,
    pub status: CaseStatus,
    pub output: Option<&'static str>,
    pub tags: Vec<&'static str>,
    pub asserts: Vec<(AssertName, AssertStatus)>,
    pub cost_usd: Option<f64>,
    pub latency_ms: u64,
    pub rendered_prompt: Option<&'static str>,
    pub error: Option<&'static str>,
    pub cached: bool,
}

impl CaseSpec {
    pub fn new(provider: &'static str, test: &'static str, status: CaseStatus) -> Self {
        CaseSpec {
            provider,
            prompt: None,
            test,
            repeat: 0,
            status,
            output: Some("hello"),
            tags: Vec::new(),
            asserts: vec![(AssertName::Contains, AssertStatus::Pass)],
            cost_usd: Some(0.0025),
            latency_ms: 42,
            rendered_prompt: None,
            error: None,
            cached: false,
        }
    }

    /// Set the case's rendered prompt to a text-style [`RenderedPrompt`]
    /// (default: none).
    pub fn rendered_prompt(mut self, text: &'static str) -> Self {
        self.rendered_prompt = Some(text);
        self
    }

    /// Set the case's provider error string (default: none).
    pub fn error(mut self, error: &'static str) -> Self {
        self.error = Some(error);
        self
    }

    pub fn output(mut self, output: Option<&'static str>) -> Self {
        self.output = output;
        self
    }

    pub fn tags(mut self, tags: Vec<&'static str>) -> Self {
        self.tags = tags;
        self
    }

    pub fn asserts(mut self, asserts: Vec<(AssertName, AssertStatus)>) -> Self {
        self.asserts = asserts;
        self
    }

    /// Set the cell's `prompt_id` (default: none).
    pub fn prompt(mut self, prompt: &'static str) -> Self {
        self.prompt = Some(prompt);
        self
    }

    /// Set the cell's `repeat` index (default: 0). Distinct repeats of the same
    /// provider/prompt/test produce distinct `case_key`s.
    pub fn repeat(mut self, repeat: u32) -> Self {
        self.repeat = repeat;
        self
    }

    /// Override the per-case cost in USD (default: `Some(0.0025)`).
    pub fn cost(mut self, cost_usd: Option<f64>) -> Self {
        self.cost_usd = cost_usd;
        self
    }

    /// Override the per-case latency in ms (default: 42).
    pub fn latency(mut self, latency_ms: u64) -> Self {
        self.latency_ms = latency_ms;
        self
    }

    /// Mark the case's provider response as a cache hit (default: false).
    pub fn cached(mut self, cached: bool) -> Self {
        self.cached = cached;
        self
    }
}

fn build_case(spec: &CaseSpec) -> CaseResult {
    let cell = CellKey {
        provider_id: spec.provider.to_string(),
        prompt_id: spec.prompt.map(|p| p.to_string()),
        test_id: spec.test.to_string(),
        repeat: spec.repeat,
    };
    let case_key = cell.case_key();
    let asserts = spec
        .asserts
        .iter()
        .map(|(kind, status)| AssertResult {
            kind: *kind,
            status: *status,
            score: if matches!(status, AssertStatus::Pass) {
                1.0
            } else {
                0.0
            },
            weight: 1.0,
            reason: String::new(),
            details: None,
            criteria: None,
            cached: false,
        })
        .collect();
    CaseResult {
        cell,
        case_key,
        name: Some(format!("{}::{}", spec.provider, spec.test)),
        tags: spec.tags.iter().map(|t| t.to_string()).collect(),
        vars: Default::default(),
        status: spec.status,
        score: if matches!(spec.status, CaseStatus::Pass) {
            1.0
        } else {
            0.0
        },
        output: spec.output.map(|o| Output::Text(o.to_string())),
        prompt: spec
            .rendered_prompt
            .map(|t| domarinn_core::types::RenderedPrompt::Text(t.to_string())),
        request: None,
        stop_reason: None,
        raw: None,
        asserts,
        usage: Some(TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: None,
        }),
        cost_usd: spec.cost_usd,
        latency_ms: spec.latency_ms,
        cached: spec.cached,
        attempts: 1,
        error: spec.error.map(str::to_string),
    }
}

/// Build a full [`RunResult`] from a set of case specs. `minute_offset` shifts
/// `finished_at` so runs can be ordered deterministically.
pub fn make_run(
    run_id: &str,
    project: Option<&str>,
    suite: Option<&str>,
    tags: Vec<&str>,
    branch: Option<&str>,
    minute_offset: i64,
    specs: &[CaseSpec],
) -> RunResult {
    let base: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let started = base + chrono::Duration::minutes(minute_offset);
    let finished = started + chrono::Duration::seconds(30);

    let cases: Vec<CaseResult> = specs.iter().map(build_case).collect();

    let mut summary = RunSummary {
        total: cases.len() as u64,
        cost_usd: Some(0.0025 * cases.len() as f64),
        ..Default::default()
    };
    for c in &cases {
        match c.status {
            CaseStatus::Pass => summary.passed += 1,
            CaseStatus::Fail => summary.failed += 1,
            CaseStatus::Error => summary.errored += 1,
            CaseStatus::Skip => summary.skipped += 1,
        }
        if let Some(u) = &c.usage {
            summary.prompt_tokens += u.input_tokens;
            summary.completion_tokens += u.output_tokens;
        }
        // Mirrors the runner's `summarize()`: every provider call is either a
        // cache hit or a miss.
        if c.cached {
            summary.cache_hits += 1;
        } else {
            summary.cache_misses += 1;
        }
    }

    RunResult {
        schema_version: domarinn_core::RESULT_SCHEMA_VERSION,
        run_id: run_id.into(),
        project: project.map(str::to_string),
        suite: suite.map(str::to_string),
        started_at: started,
        finished_at: finished,
        config_digest: "sha256:deadbeef".to_string(),
        config_snapshot: serde_json::json!({"providers": [], "tests": []}),
        git: branch.map(|b| GitMeta {
            branch: Some(b.to_string()),
            commit: Some("abc123".to_string()),
            dirty: false,
        }),
        ci: Some(CiMeta {
            provider: Some("ci".to_string()),
            run_url: Some("https://ci.example/run/1".to_string()),
        }),
        filters: FilterSpec {
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        },
        cases,
        summary,
    }
}

/// A tiny run with a single passing case.
/// An unfiltered `CaseListFilter` for a run — the storage-level equivalent of
/// `GET /runs/{id}/cases` with no query params.
pub fn default_case_filter(run_id: RunId) -> CaseListFilter {
    CaseListFilter {
        run_id,
        status: None,
        tag: None,
        q: None,
        provider: None,
        prompt: None,
        test: None,
        stop_reason: None,
        cached: None,
        limit: 200,
        cursor: None,
    }
}

pub fn simple_run(run_id: &str) -> RunResult {
    make_run(
        run_id,
        Some("proj"),
        Some("suite"),
        vec!["nightly"],
        Some("main"),
        0,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    )
}

pub fn run_value(run: &RunResult) -> Value {
    serde_json::to_value(run).unwrap()
}
