//! The exec JSON protocol shared by providers, asserts, and generators.
//!
//! The wire types themselves live in the standalone `domarinn-protocol` crate —
//! serde and nothing else — so a third-party provider can depend on them
//! without inheriting tokio, reqwest, and minijinja. This module is the
//! in-engine name for them, so `crate::exec_protocol::*` keeps resolving.
//!
//! Add fields there, not here. See `crates/domarinn-protocol/README.md` for why
//! this is separate from `domarinn-types`, and `docs/protocol.md` for the
//! normative specification.

pub use domarinn_protocol::*;
