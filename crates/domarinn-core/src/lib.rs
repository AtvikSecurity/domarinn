//! domarinn-core — the eval engine, config schema, and traits.
//!
//! This crate is a pure library: no terminal I/O, no network clients. It defines
//! the shapes the CLI and server share ([`RunResult`], the exec protocol) and the
//! extension seams ([`Provider`], [`Assertion`], [`CacheBackend`]).

pub mod anthropic;
pub mod assertion;
pub mod asserts;
pub mod cache;
pub mod cache_key;
pub mod config;
pub mod diff;
pub mod digests;
pub mod embeddings;
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
pub mod loader;
pub mod loader_validate;
pub mod matrix;
pub mod net;
pub mod openai;
pub mod progress;
pub mod provenance;
pub mod provider;
pub mod provider_factory;
pub mod render;
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
pub use loader::{load_file, load_str, validate, Issue};
pub use progress::{ProgressEvent, ProgressSink};
pub use resolve::{expand_tests, Expanded};
pub use result::{RunResult, RESULT_SCHEMA_VERSION};
pub use runner::{run, run_with_progress, AssertGrader, RunOptions};
pub use template::TemplateEngine;
pub use val::Val;

/// The crate version, recorded in cache entries and run metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Generate the JSON Schema for the suite config.
pub fn config_schema() -> serde_json::Value {
    let schema = schemars::schema_for!(config::Suite);
    serde_json::to_value(schema).expect("schema serializes")
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
