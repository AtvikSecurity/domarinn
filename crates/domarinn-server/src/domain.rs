//! Enums and id newtypes shared across the HTTP and SQL boundaries.
//!
//! [`Role`] and [`crate::auth::Scope`] cross both boundaries: they are stored
//! in SQL (via [`rusqlite::types::ToSql`]/[`rusqlite::types::FromSql`] on the
//! direct-rusqlite path, and via the executor's `IntoValue`/`FromValue` pair
//! over its owned `Value` on the backend-neutral `crate::storage::exec` one)
//! and appear in JSON request/response bodies (via `serde`). Every enum here
//! serializes to the exact lowercase strings the wire format already used as
//! plain strings before this module existed — see the server integration
//! tests for the frozen JSON assertions.
//!
//! `Role`'s and `Scope`'s SQL glue is kept together in this module even
//! though `Scope` itself is defined in [`crate::auth`] (it needs the
//! `Read`/`Write`/`Admin` ordering that lives alongside the auth logic).
//!
//! [`UserId`] and [`ApiKeyId`] are the server-local counterparts of core's
//! [`domarinn_core::ids::RunId`]/[`domarinn_core::ids::CaseKey`]: opaque,
//! `#[serde(transparent)]` string newtypes (see that module's docs for why
//! `transparent` is load-bearing) with a `ToSql`/`FromSql` pair (plus the
//! executor's `IntoValue`/`FromValue` twin) so storage code binds them
//! directly instead of threading `.as_str()` through every call site. The macro is a local copy rather than a re-export from core —
//! core stays rusqlite-free (orphan rule), and these two ids are server-only.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::auth::Scope;
use crate::storage::exec::{FromValue, IntoValue, Value};

macro_rules! string_id {
    ($(#[$meta:meta])* pub struct $name:ident;) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wrap an existing id verbatim. No format validation: ids are
            /// opaque, client-supplied strings.
            pub fn new(id: impl Into<String>) -> Self {
                $name(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok($name::new(s))
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                $name(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                $name::new(s)
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                Ok($name::new(value.as_str()?))
            }
        }

        impl crate::storage::exec::IntoValue for $name {
            fn to_value(&self) -> crate::storage::exec::Value {
                crate::storage::exec::Value::Text(self.as_str().to_owned())
            }
        }

        impl crate::storage::exec::FromValue for $name {
            fn from_value(v: crate::storage::exec::Value) -> anyhow::Result<Self> {
                Ok($name::new(
                    <String as crate::storage::exec::FromValue>::from_value(v)?,
                ))
            }
        }
    };
}

string_id!(
    /// A local account's id.
    pub struct UserId;
);

string_id!(
    /// An API key's id (distinct from the key secret and from its hash).
    pub struct ApiKeyId;
);

/// A local account's role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
    /// Read-only: sees runs and cases, writes nothing.
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Viewer => "viewer",
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
            "viewer" => Ok(Role::Viewer),
            other => Err(format!(
                "invalid role '{other}'; expected one of: admin, member, viewer"
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

impl IntoValue for Role {
    fn to_value(&self) -> Value {
        Value::Text(self.as_str().to_owned())
    }
}

impl FromValue for Role {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        String::from_value(v)?.parse().map_err(anyhow::Error::msg)
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

impl IntoValue for Scope {
    fn to_value(&self) -> Value {
        Value::Text(self.label().to_owned())
    }
}

impl FromValue for Scope {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        String::from_value(v)?.parse().map_err(anyhow::Error::msg)
    }
}

/// Which SSO protocol an identity or login transaction belongs to. Stored in
/// SQLite (`user_identities.kind`, `login_transactions.kind`) and serialized
/// into identity views on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SsoKind {
    Oidc,
    Saml,
}

impl SsoKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SsoKind::Oidc => "oidc",
            SsoKind::Saml => "saml",
        }
    }
}

impl std::fmt::Display for SsoKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SsoKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "oidc" => Ok(SsoKind::Oidc),
            "saml" => Ok(SsoKind::Saml),
            other => Err(format!(
                "invalid sso kind '{other}'; expected one of: oidc, saml"
            )),
        }
    }
}

impl ToSql for SsoKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for SsoKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|e: String| FromSqlError::Other(e.into()))
    }
}

impl IntoValue for SsoKind {
    fn to_value(&self) -> Value {
        Value::Text(self.as_str().to_owned())
    }
}

impl FromValue for SsoKind {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        String::from_value(v)?.parse().map_err(anyhow::Error::msg)
    }
}

/// Status filter for `GET /runs?status=`. A narrower set than
/// [`domarinn_core::result::CaseStatus`]: `skip`ped cases never move a
/// run's pass/fail/error counters, so a run-level filter for `skip` would
/// never match anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum RunStatusFilter {
    Pass,
    Fail,
    Error,
}

/// Cache filter for `GET /runs?cached=`. `exclude` hides runs that were fully
/// cached AND passing — a fully-cached run can still fail, and those stay
/// visible; `only` returns fully-cached runs regardless of verdict; `all` is
/// the explicit no-op default.
///
/// `cached` here counts provider responses, which is what `RunSummary.cache_hits`
/// tracks. Grader verdicts are cached too, but separately: a run can replay
/// every provider response and every verdict and still be a `fail`, because
/// the *stored* verdict is what failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum CachedFilter {
    Exclude,
    Only,
    All,
}

/// Origin filter for `GET /runs?origin=` — the facet that separates the CI
/// stream from developer iteration.
///
/// Keyed on `ci_provider IS NOT NULL`, which is exact: `provenance::collect_ci`
/// returns a provider (`"ci"`) even for a bare `CI` environment variable, so
/// there is no run that ran in CI without one. Runs uploaded before provenance
/// moved into the engine have no `ci_provider` unless they were shared from CI,
/// and so read as `local` — which is what they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum OriginFilter {
    Ci,
    Local,
}

/// Which cache a browse request is about.
///
/// `Server` is this instance's `cache.db` — the shared tier a `layered` run
/// writes to. `Local` is a read-only disk cache directory mounted by the
/// operator, which is what a single developer's runs actually fill, since
/// `cache.backend` defaults to `disk`. A tier that is not mounted is a `404`
/// rather than a `400`: the request is well-formed, the resource is simply not
/// present on this instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum CacheTier {
    #[default]
    Server,
    Local,
}

/// Sort column for `GET /cache/entries?sort=`.
///
/// Deliberately only columns SQL can order on directly — for `kind`, `model`
/// and the token counts that means the promoted migration-11 columns, not the
/// entry body. Everything else a row shows lives inside the body, and sorting
/// a single page of a large cache by a field the database cannot see would be
/// a lie dressed as an ordering — `CaseGrid`'s "sorted within loaded cases"
/// apology is the shape to avoid, not to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CacheSort {
    #[default]
    Created,
    LastAccess,
    Size,
    Cost,
    Kind,
    Model,
    /// `input_tokens + output_tokens`, the same sum the Tokens column renders.
    Tokens,
    Key,
}

/// Newest / largest / most expensive first, which is what someone opening a
/// cache browser is looking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Asc,
    #[default]
    Desc,
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
    fn user_id_round_trips_through_rusqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id TEXT NOT NULL)")
            .unwrap();
        let id = UserId::new("usr_abc123");
        conn.execute("INSERT INTO t (id) VALUES (?1)", rusqlite::params![id])
            .unwrap();
        let got: UserId = conn
            .query_row("SELECT id FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got, id);
    }

    #[test]
    fn api_key_id_round_trips_through_rusqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id TEXT NOT NULL)")
            .unwrap();
        let id = ApiKeyId::new("key_xyz789");
        conn.execute("INSERT INTO t (id) VALUES (?1)", rusqlite::params![id])
            .unwrap();
        let got: ApiKeyId = conn
            .query_row("SELECT id FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got, id);
    }

    #[test]
    fn role_round_trips_through_exec_value() {
        for role in [Role::Admin, Role::Member, Role::Viewer] {
            assert_eq!(Role::from_value(role.to_value()).unwrap(), role);
        }
    }

    #[test]
    fn scope_round_trips_through_exec_value() {
        for scope in [Scope::Read, Scope::Write, Scope::Admin] {
            assert_eq!(Scope::from_value(scope.to_value()).unwrap(), scope);
        }
    }

    #[test]
    fn sso_kind_round_trips_through_exec_value() {
        for kind in [SsoKind::Oidc, SsoKind::Saml] {
            assert_eq!(SsoKind::from_value(kind.to_value()).unwrap(), kind);
        }
    }

    #[test]
    fn user_id_round_trips_through_exec_value() {
        let id = UserId::new("usr_abc123");
        assert_eq!(UserId::from_value(id.to_value()).unwrap(), id);
    }

    #[test]
    fn api_key_id_round_trips_through_exec_value() {
        let id = ApiKeyId::new("key_xyz789");
        assert_eq!(ApiKeyId::from_value(id.to_value()).unwrap(), id);
    }

    #[test]
    fn invalid_exec_value_scope_fails_to_parse() {
        // Mirrors `invalid_stored_scope_fails_to_parse`: garbage must error,
        // never default.
        assert!(Scope::from_value(Value::Text("bogus".into())).is_err());
    }

    #[test]
    fn user_id_serializes_as_a_bare_string_not_an_object() {
        let id = UserId::new("usr_abc123");
        let json = serde_json::to_value(&id).unwrap();
        assert_eq!(json, serde_json::json!("usr_abc123"));
        let back: UserId = serde_json::from_value(json).unwrap();
        assert_eq!(back, id);
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
        assert!(err.contains("viewer"));
    }

    /// Every representation of a role has to agree, or a role stored by one
    /// path becomes unreadable by another: `viewer` is the third variant and
    /// the first added after the impls were written.
    #[test]
    fn every_role_agrees_across_sql_serde_and_from_str() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (role TEXT NOT NULL)")
            .unwrap();
        for role in [Role::Admin, Role::Member, Role::Viewer] {
            conn.execute("DELETE FROM t", []).unwrap();
            conn.execute("INSERT INTO t (role) VALUES (?1)", rusqlite::params![role])
                .unwrap();
            let got: Role = conn
                .query_row("SELECT role FROM t", [], |r| r.get(0))
                .unwrap();
            assert_eq!(got, role);
            assert_eq!(
                serde_json::to_value(role).unwrap(),
                serde_json::json!(role.as_str())
            );
            assert_eq!(role.as_str().parse::<Role>().unwrap(), role);
            assert_eq!(role.to_string(), role.as_str());
        }
    }
}
