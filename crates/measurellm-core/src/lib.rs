//! measurellm-core — the eval engine, config schema, and traits.
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
pub mod embeddings;
pub mod exec;
pub mod exec_protocol;
pub mod exec_provider;
pub mod filter;
pub mod generate;
pub mod grader;
pub mod http_provider;
pub mod ids;
pub mod loader;
pub mod net;
pub mod openai;
pub mod provider;
pub mod provider_factory;
pub mod render;
pub mod resolve;
pub mod result;
pub mod runner;
pub mod scoring;
pub mod stats;
pub mod template;
pub mod types;
pub mod val;

pub use config::Suite;
pub use diff::{diff_runs, RunDiff};
pub use filter::{Filter, FilterOpts};
pub use grader::DefaultGrader;
pub use ids::{CaseKey, RunId};
pub use loader::{load_file, load_str, validate, Issue};
pub use resolve::{expand_tests, Expanded};
pub use result::{RunResult, RESULT_SCHEMA_VERSION};
pub use runner::{run, AssertGrader, RunOptions};
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
