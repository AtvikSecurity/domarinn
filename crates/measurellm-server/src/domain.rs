//! Enums shared across the HTTP and SQLite boundaries.
//!
//! [`Role`] and [`crate::auth::Scope`] cross both boundaries: they are stored
//! in SQLite (via [`rusqlite::types::ToSql`]/[`rusqlite::types::FromSql`]) and
//! appear in JSON request/response bodies (via `serde`). Every enum here
//! serializes to the exact lowercase strings the wire format already used as
//! plain strings before this module existed — see the server integration
//! tests for the frozen JSON assertions.
//!
//! `Role`'s and `Scope`'s SQL glue is kept together in this module even
//! though `Scope` itself is defined in [`crate::auth`] (it needs the
//! `Read`/`Write`/`Admin` ordering that lives alongside the auth logic).

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::auth::Scope;

/// A local account's role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Role::Admin),
            "member" => Ok(Role::Member),
            other => Err(format!(
                "invalid role '{other}'; expected one of: admin, member"
            )),
        }
    }
}

impl ToSql for Role {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for Role {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|e: String| FromSqlError::Other(e.into()))
    }
}

impl ToSql for Scope {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.label().into())
    }
}

impl FromSql for Scope {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|e: String| FromSqlError::Other(e.into()))
    }
}

/// Status filter for `GET /runs?status=`. A narrower set than
/// [`measurellm_core::result::CaseStatus`]: `skip`ped cases never move a
/// run's pass/fail/error counters, so a run-level filter for `skip` would
/// never match anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum RunStatusFilter {
    Pass,
    Fail,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn role_round_trips_through_rusqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (role TEXT NOT NULL)")
            .unwrap();
        conn.execute(
            "INSERT INTO t (role) VALUES (?1)",
            rusqlite::params![Role::Admin],
        )
        .unwrap();
        let got: Role = conn
            .query_row("SELECT role FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got, Role::Admin);
    }

    #[test]
    fn scope_round_trips_through_rusqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (scope TEXT NOT NULL)")
            .unwrap();
        conn.execute(
            "INSERT INTO t (scope) VALUES (?1)",
            rusqlite::params![Scope::Write],
        )
        .unwrap();
        let got: Scope = conn
            .query_row("SELECT scope FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got, Scope::Write);
    }

    #[test]
    fn invalid_stored_scope_fails_to_parse() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (scope TEXT NOT NULL)")
            .unwrap();
        // No CHECK constraint on api_keys.scope in the real schema, so a
        // manually-inserted garbage value is the only way this can happen —
        // simulate it directly here.
        conn.execute("INSERT INTO t (scope) VALUES ('bogus')", [])
            .unwrap();
        let err = conn
            .query_row("SELECT scope FROM t", [], |r| r.get::<_, Scope>(0))
            .unwrap_err();
        assert!(matches!(
            err,
            rusqlite::Error::FromSqlConversionFailure(_, _, _)
        ));
    }

    #[test]
    fn role_from_str_error_names_bad_value_and_valid_set() {
        let err = "superadmin".parse::<Role>().unwrap_err();
        assert!(err.contains("superadmin"));
        assert!(err.contains("admin"));
        assert!(err.contains("member"));
    }
}
