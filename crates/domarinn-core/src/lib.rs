//! domarinn-core — the eval engine, config schema, and traits.
//!
//! This crate is a pure library: no terminal I/O, no network clients. It defines
//! the shapes the CLI and server share ([`RunResult`], the exec protocol) and the
//! extension seams ([`Provider`], [`Assertion`], [`CacheBackend`]).

pub mod anthropic;
pub mod assertion;
pub mod asserts;
pub mod cache;
pub mod cache_adopt;
pub mod cache_key;
pub mod cache_migrate;
pub mod chat_wire;
pub mod composite;
pub mod config;
pub mod config_history;
pub mod config_provider;
pub mod config_request;
pub mod diff;
pub mod digests;
pub mod embeddings;
pub mod empty_policy;
pub mod empty_run;
pub mod errors;

// The wire contract lives in `domarinn-types`. Re-exported module-for-module so
// `domarinn_core::result::RunResult` and friends keep resolving: the split is an
// internal reorganisation, not a rename for every caller in the workspace.
pub use domarinn_types::{empty, error_class, ids, result, types};

// The exec protocol lives in `domarinn-protocol`, a serde-only crate an
// external provider author can depend on. Re-exported under its own name so
// embedders can reach it without guessing the path.
pub use domarinn_protocol;
pub mod exec;
pub mod exec_protocol;
pub mod exec_provider;
pub mod filevars;
pub mod filter;
pub mod generate;
pub mod grader;
pub mod http_provider;
pub mod interp;
pub mod jsonschema_cache;
pub mod loader;
pub mod loader_validate;
pub mod loader_validate_fallback;
pub mod loader_validate_history;
pub mod matrix;
pub mod net;
pub mod openai;
pub mod preflight;
pub mod pricing;
pub mod progress;
pub mod provenance;
pub mod provider;
pub mod provider_factory;
pub mod render;
mod request_cache;
pub mod request_cfg;
pub mod resolve;
pub mod retry;
pub mod runner;
pub mod sandbox;
pub mod scoring;
pub mod stats;
pub mod template;
pub mod template_fns;
pub mod val;

pub use config::Suite;
pub use diff::{diff_runs, RunDiff};
pub use filter::{Filter, FilterOpts};
pub use grader::DefaultGrader;
pub use ids::{CaseKey, RunId};
pub use loader::{load_file, load_str, validate, Issue, Severity, Validation};
pub use progress::{ProgressEvent, ProgressSink};
pub use resolve::{expand_tests, Expanded};
pub use result::{RunResult, RESULT_SCHEMA_VERSION};
pub use runner::{run, run_with_progress, AssertGrader, RunOptions};
pub use template::TemplateEngine;
pub use val::Val;

/// The crate version, recorded in cache entries and run metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Generate the JSON Schema for the suite config.
/// The canonical location of the generated config schema.
///
/// Deliberately version-less. Embedding [`VERSION`] would make `schema-check`
/// diff-fail on every standing release PR: release-please bumps the workspace
/// version on the release branch, and the lockfile-sync job that follows runs
/// `cargo update --workspace` and nothing else — so the committed schema would
/// never be regenerated to match.
///
/// A pinned per-release copy is still addressable, because the file is
/// committed at the repository root and tags are bare semver:
/// `https://raw.githubusercontent.com/AtvikSecurity/domarinn/<version>/domarinn.schema.json`.
pub const CONFIG_SCHEMA_ID: &str =
    "https://raw.githubusercontent.com/AtvikSecurity/domarinn/main/domarinn.schema.json";

pub fn config_schema() -> serde_json::Value {
    let schema = schemars::schema_for!(config::Suite);
    let value = serde_json::to_value(schema).expect("schema serializes");
    // Added after generation rather than via a schemars attribute so the
    // constant stays the single place the URL is written.
    // `schemars` is built with `preserve_order`, so a plain insert would append
    // `$id` after 1400 lines of definitions. Rebuilt with it near the front,
    // where a reader looks for it.
    if let Some(obj) = value.as_object() {
        let mut out = serde_json::Map::new();
        if let Some(dialect) = obj.get("$schema") {
            out.insert("$schema".into(), dialect.clone());
        }
        out.insert(
            "$id".into(),
            serde_json::Value::String(CONFIG_SCHEMA_ID.into()),
        );
        for (k, v) in obj {
            if k != "$schema" {
                out.insert(k.clone(), v.clone());
            }
        }
        return serde_json::Value::Object(out);
    }
    value
}

/// Generate the JSON Schema for [`RunResult`].
pub fn result_schema() -> serde_json::Value {
    let schema = schemars::schema_for!(result::RunResult);
    serde_json::to_value(schema).expect("schema serializes")
}

/// Export TypeScript type definitions for the result and diff DTOs to `dir`.
///
/// This is the single source of truth for the web client's types.
pub fn export_types(dir: &std::path::Path) -> Result<(), ts_rs::ExportError> {
    use ts_rs::{Config, TS};
    // u64/i64 export as `number`: token counts / latency will not exceed 2^53 in
    // practice, and `bigint` is unusable against `JSON.parse` output on the wire.
    let cfg = Config::new().with_out_dir(dir).with_large_int("number");
    result::RunResult::export_all(&cfg)?;
    diff::RunDiff::export_all(&cfg)?;
    Ok(())
}
