//! Response DTOs for the server's read/compare/ingest API.
//!
//! Every type here derives `Serialize` + `TS` and serializes to the exact JSON
//! shape the handlers returned as hand-built `serde_json::json!` blobs before
//! this module existed — the wire format is frozen; see the integration tests
//! under `tests/` and the wire-shape pin tests in each submodule. `TS` makes
//! each struct/field definition the source of truth for the generated
//! TypeScript types the web app imports (wired up in a later task; this crate
//! does not call `TS::export` anywhere itself).
//!
//! Split by the storage submodule each DTO family serves:
//! * [`accounts`] — local accounts, sessions, and API keys (`/auth/*`,
//!   `/apikeys`, `/users`).
//! * [`runs`] — run list/detail items and the lean per-case assert record
//!   stored in the `cases.asserts` DB column.
//! * [`cases`] — case list items and the case detail envelope.
//! * [`compare`] — the run/run comparison response.
//! * [`projects`] — projects, suites, and the suite pass-rate series.
//! * [`cache`] — cache stats and prune responses.
//! * [`meta`] — the `/api/v1/meta` response.

pub mod accounts;
pub mod cache;
pub mod cases;
pub mod compare;
pub mod meta;
pub mod projects;
pub mod runs;
