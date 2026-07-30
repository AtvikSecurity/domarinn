//! Response DTOs for the server's read/compare/ingest API.
//!
//! Every type here derives `Serialize` + `TS` and serializes to the exact JSON
//! shape the handlers returned as hand-built `serde_json::json!` blobs before
//! this module existed — the wire format is frozen; see the integration tests
//! under `tests/` and the wire-shape pin tests in each submodule. `TS` makes
//! each struct/field definition the source of truth for the generated
//! TypeScript types the web app imports: [`crate::export_api_types`] calls
//! `TS::export_all` for every response DTO and request body reachable from
//! the API, and `domarinn-cli gen-types` invokes it to regenerate
//! `web/src/api/generated/`.
//!
//! Split by the storage submodule each DTO family serves:
//! * [`accounts`] — local accounts, sessions, and API keys (`/auth/*`,
//!   `/apikeys`, `/users`).
//! * [`runs`] — run list/detail items and the lean per-case assert record
//!   stored in the `cases.asserts` DB column.
//! * [`cases`] — case list items and the case detail envelope.
//! * [`compare`] — the run/run comparison response.
//! * [`config`] — the `GET /runs/{id}/config` digest + snapshot response.
//! * [`history`] — one case's evolution across a suite's recent runs.
//! * [`matrix`] — the per-run prompt × provider aggregate matrix.
//! * [`projects`] — projects, suites, and the suite pass-rate series.
//! * [`cache`] — cache stats and prune responses.
//! * [`meta`] — the `/api/v1/meta` response.
//! * [`search`] — the `/api/v1/search` grouped full-text hits.

pub mod accounts;
pub mod cache;
pub mod cacheentries;
pub mod cases;
pub mod compare;
pub mod config;
pub mod history;
pub mod matrix;
pub mod meta;
pub mod projects;
pub mod runs;
pub mod search;
