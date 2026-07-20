//! Cache backend implementations for measurellm.
//!
//! The [`measurellm_core::cache::CacheBackend`] trait lives in core; this crate
//! provides the concrete stores:
//!
//! - [`LocalDiskCache`] — content-addressed files on the local filesystem.
//! - [`RemoteHttpCache`] — the measurellm server's HTTP cache endpoints.
//! - [`S3Cache`] — any S3-compatible object store (AWS, MinIO, Garage, ...).
//! - [`LayeredCache`] — a read-through pairing of a fast local and a shared
//!   remote backend.

mod disk;
mod layered;
mod remote_http;
mod s3;

pub use disk::LocalDiskCache;
pub use layered::LayeredCache;
pub use remote_http::RemoteHttpCache;
pub use s3::{S3Cache, S3Config};
