//! measurellm-core — the eval engine, config schema, and traits.
//!
//! This crate is a pure library: no terminal I/O, no network clients. It defines
//! the shapes the CLI and server share ([`RunResult`], the exec protocol) and the
//! extension seams ([`Provider`], [`Assertion`], [`CacheBackend`]).

pub mod assertion;
pub mod cache;
pub mod config;
pub mod exec_protocol;
pub mod loader;
pub mod provider;
pub mod result;
pub mod template;
pub mod types;
pub mod val;

pub use config::Suite;
pub use loader::{load_file, load_str, validate, Issue};
pub use result::{RunResult, RESULT_SCHEMA_VERSION};
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
