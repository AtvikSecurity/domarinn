//! Cache backend implementations for domarinn.
//!
//! The [`domarinn_core::cache::CacheBackend`] trait lives in core; this crate
//! provides the concrete stores:
//!
//! - [`LocalDiskCache`] — content-addressed files on the local filesystem.
//! - [`RemoteHttpCache`] — the domarinn server's HTTP cache endpoints.
//! - [`S3Cache`] — any S3-compatible object store (AWS, MinIO, Garage, ...).
//! - [`LayeredCache`] — a read-through pairing of a fast local and a shared
//!   remote backend.
//! - [`ReadOnlyCache`] — an adapter that serves reads and discards writes, for
//!   draining a store rather than depending on it.
//!
//! The network backends are behind default-on features so a consumer that only
//! needs local disk — the server, for its read-only browse tier — can take this
//! crate without `object_store` and the AWS stack it brings.

mod disk;
mod layered;
mod readonly;
#[cfg(feature = "remote-http")]
mod remote_http;
#[cfg(feature = "s3")]
mod s3;

pub use disk::LocalDiskCache;
pub use layered::LayeredCache;
pub use readonly::ReadOnlyCache;
#[cfg(feature = "remote-http")]
pub use remote_http::RemoteHttpCache;
#[cfg(feature = "s3")]
pub use s3::{S3Cache, S3Config};
