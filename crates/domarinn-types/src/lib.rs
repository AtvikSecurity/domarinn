//! domarinn's wire types: the run document and the vocabulary shared by the
//! engine, the CLI, the server, and the web UI.
//!
//! Split out of `domarinn-core` so the contract is separable from the machinery
//! that produces it. Three things follow from that:
//!
//! - The server can depend on the wire shapes without pulling in the engine's
//!   `reqwest`/`minijinja`/`tokio` tree.
//! - The ts-rs export surface is explicit: everything the web UI can see is a
//!   type in this crate (plus the server's own response DTOs), rather than
//!   whatever happened to derive `TS` somewhere in a 40-module crate.
//! - Changing a stored shape is a visible, deliberate edit to a small crate
//!   whose whole purpose is compatibility — see the byte-stability rules on
//!   [`result::RunResult`].
//!
//! `domarinn-core` re-exports every module here, so `domarinn_core::result::X`
//! and friends keep resolving for existing callers.

pub mod assert_name;
pub mod change;
pub mod empty;
pub mod error_class;
pub mod ids;
pub mod result;
pub mod types;

pub use result::{RunResult, RESULT_SCHEMA_VERSION};
