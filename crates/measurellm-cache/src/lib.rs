//! Cache backend implementations for measurellm.
//!
//! The [`measurellm_core::cache::CacheBackend`] trait lives in core; this crate
//! provides the concrete stores. Phase 0 ships [`LocalDiskCache`]; remote-HTTP,
//! S3, and layered backends land in later phases.

mod disk;

pub use disk::LocalDiskCache;
