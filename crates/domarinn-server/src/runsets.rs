//! Run-set access control: who may read and write which `(project, suite)`.
//!
//! # The model
//!
//! Default-open. A `(project, suite)` pair is **restricted** only when a
//! `run_set_restrictions` row covers it: a row `(P, NULL)` covers every suite in
//! project `P`, a row `(P, S)` covers just that suite. A `run_set_grants` row
//! covers a run the same way, at one of three ordered [`GrantLevel`]s.
//!
//! A run is visible iff it is **not restricted, or the caller holds any grant
//! covering it**. Runs with a NULL `project` can never be restricted (no
//! restriction row can name them), so they are always visible — a property SQL
//! gives for free, and [`visibility_predicate`] relies on: `rsr.project =
//! runs.project` is NULL, never true, when `runs.project` is NULL.
//!
//! # Why the derivation lives here
//!
//! [`RunVisibility::of`] is the *only* place an [`Identity`] becomes an access
//! class. A second copy would be a security bug waiting to happen — the same
//! reason [`crate::mcp::dispatch`] reuses [`Identity::check`] rather than
//! re-implementing the auth-mode matrix.
//!
//! Note what is deliberately *not* privileged: a non-admin static token
//! (`DOMARINN_TOKENS`) resolves to [`RunVisibility::Public`]. Those are shared,
//! widely-copied credentials with no owning user and therefore no grants; if
//! they pierced restrictions, every CI job in the deployment would.
//!
//! [`GrantLevel`] lives here rather than in [`crate::domain`] so the whole
//! access model — levels, classes, and the predicate that joins them — reads in
//! one file. Its SQL glue mirrors [`crate::domain::Role`]'s.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::auth::{Identity, IdentitySource, Scope};
use crate::domain::{Role, UserId};
use crate::storage::exec::{FromValue, IntoValue, Value};

/// What a grant lets its holder do with a restricted run set. Ordered:
/// `View < Upload < Manage`, and a higher level includes everything below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum GrantLevel {
    /// Read the set's runs.
    View = 1,
    /// Read, and upload runs into it.
    Upload = 2,
    /// Read, upload, and administer the set's own grants.
    Manage = 3,
}

impl GrantLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            GrantLevel::View => "view",
            GrantLevel::Upload => "upload",
            GrantLevel::Manage => "manage",
        }
    }
}

impl std::fmt::Display for GrantLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for GrantLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "view" => Ok(GrantLevel::View),
            "upload" => Ok(GrantLevel::Upload),
            "manage" => Ok(GrantLevel::Manage),
            other => Err(format!(
                "invalid grant level '{other}'; expected one of: view, upload, manage"
            )),
        }
    }
}

impl ToSql for GrantLevel {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for GrantLevel {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|e: String| FromSqlError::Other(e.into()))
    }
}

impl IntoValue for GrantLevel {
    fn into_value(&self) -> Value {
        Value::Text(self.as_str().to_owned())
    }
}

impl FromValue for GrantLevel {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        String::from_value(v)?.parse().map_err(anyhow::Error::msg)
    }
}

/// Which runs a caller may see. Derived once, by [`RunVisibility::of`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunVisibility {
    /// Sees everything: an admin account (via any credential), or a static
    /// token carrying [`Scope::Admin`].
    Full,
    /// A user-backed identity — session or API key. API keys ride the owning
    /// user's grants, so both resolve to the same class.
    User(UserId),
    /// Anonymous callers and non-admin static tokens: unrestricted sets only.
    Public,
}

impl RunVisibility {
    /// The caller's access class. The single derivation in the crate.
    pub fn of(identity: &Identity) -> RunVisibility {
        if identity.role == Some(Role::Admin) {
            return RunVisibility::Full;
        }
        if identity.source == IdentitySource::Static && identity.scope == Some(Scope::Admin) {
            return RunVisibility::Full;
        }
        match &identity.user_id {
            Some(user_id) => RunVisibility::User(user_id.clone()),
            None => RunVisibility::Public,
        }
    }
}

/// SQL that is true for a row of `runs` (aliased `alias`) this caller may see.
///
/// `alias` is always a compile-time table alias chosen by the caller, never
/// user input. The user id — the only value that varies per request — is bound
/// as a positional parameter, pushed onto `args`, and referenced as
/// `?{args.len()}` in the returned SQL. That mirrors the dynamic-SQL builder
/// style in [`crate::storage`]'s run list.
///
/// [`RunVisibility::Full`] yields `1=1` rather than nothing, so every call site
/// can append the clause unconditionally instead of branching.
pub(crate) fn visibility_predicate(
    alias: &str,
    vis: &RunVisibility,
    args: &mut Vec<Value>,
) -> String {
    // NULL-project runs fall out visible for free: `rsr.project = {alias}.project`
    // is NULL (never true) when the column is NULL, so the subquery matches
    // nothing and `NOT EXISTS` holds.
    let unrestricted = format!(
        "NOT EXISTS (SELECT 1 FROM run_set_restrictions rsr \
           WHERE rsr.project = {alias}.project \
             AND (rsr.suite IS NULL OR rsr.suite = {alias}.suite))"
    );
    match vis {
        RunVisibility::Full => "1=1".to_string(),
        RunVisibility::Public => unrestricted,
        RunVisibility::User(user_id) => {
            args.push(Value::Text(user_id.as_str().to_string()));
            let p = args.len();
            format!(
                "({unrestricted} OR EXISTS (SELECT 1 FROM run_set_grants g \
                   WHERE g.user_id = ?{p} AND g.project = {alias}.project \
                     AND (g.suite IS NULL OR g.suite = {alias}.suite)))"
            )
        }
    }
}

/// SQL that is true when the run bound to parameter `?{run_param}` exists *and*
/// is visible to this caller.
///
/// The `EXISTS` form is for the queries keyed on a run id whose own table has
/// no `project`/`suite` columns to filter on — cases, matrix rows, stored
/// blobs. `?{run_param}` must already be bound by the caller; the predicate's
/// own parameter (if any) is appended after it.
pub(crate) fn visible_run_predicate(
    run_param: usize,
    vis: &RunVisibility,
    args: &mut Vec<Value>,
) -> String {
    let clause = visibility_predicate("vr", vis, args);
    format!("EXISTS (SELECT 1 FROM runs vr WHERE vr.id = ?{run_param} AND {clause})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthMode;
    use rusqlite::Connection;

    fn anon() -> Identity {
        Identity::anonymous(AuthMode::Closed)
    }

    fn static_token(scope: Scope) -> Identity {
        Identity {
            scope: Some(scope),
            source: IdentitySource::Static,
            label: Some(scope.label().to_string()),
            ..anon()
        }
    }

    fn account(role: Role, source: IdentitySource) -> Identity {
        Identity {
            scope: Some(Scope::for_role(role)),
            source,
            user_id: Some(UserId::new("usr_1")),
            username: Some("someone".to_string()),
            role: Some(role),
            ..anon()
        }
    }

    #[test]
    fn grant_levels_are_ordered_and_round_trip() {
        assert!(GrantLevel::Manage > GrantLevel::Upload);
        assert!(GrantLevel::Upload > GrantLevel::View);

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (level TEXT NOT NULL)")
            .unwrap();
        for level in [GrantLevel::View, GrantLevel::Upload, GrantLevel::Manage] {
            conn.execute("DELETE FROM t", []).unwrap();
            conn.execute(
                "INSERT INTO t (level) VALUES (?1)",
                rusqlite::params![level],
            )
            .unwrap();
            let got: GrantLevel = conn
                .query_row("SELECT level FROM t", [], |r| r.get(0))
                .unwrap();
            assert_eq!(got, level);
            assert_eq!(
                serde_json::to_value(level).unwrap(),
                serde_json::json!(level.as_str())
            );
            assert_eq!(level.as_str().parse::<GrantLevel>().unwrap(), level);
        }
        assert!("owner".parse::<GrantLevel>().is_err());
    }

    #[test]
    fn grant_level_round_trips_through_exec_value() {
        for level in [GrantLevel::View, GrantLevel::Upload, GrantLevel::Manage] {
            assert_eq!(GrantLevel::from_value(level.into_value()).unwrap(), level);
        }
        assert!(GrantLevel::from_value(Value::Text("owner".into())).is_err());
    }

    #[test]
    fn admins_of_any_source_see_everything() {
        for source in [IdentitySource::Session, IdentitySource::ApiKey] {
            assert_eq!(
                RunVisibility::of(&account(Role::Admin, source)),
                RunVisibility::Full,
                "{source:?}"
            );
        }
        assert_eq!(
            RunVisibility::of(&static_token(Scope::Admin)),
            RunVisibility::Full
        );
    }

    #[test]
    fn members_and_viewers_ride_their_own_grants() {
        for role in [Role::Member, Role::Viewer] {
            assert_eq!(
                RunVisibility::of(&account(role, IdentitySource::Session)),
                RunVisibility::User(UserId::new("usr_1")),
                "{role}"
            );
            assert_eq!(
                RunVisibility::of(&account(role, IdentitySource::ApiKey)),
                RunVisibility::User(UserId::new("usr_1")),
                "{role} api key"
            );
        }
    }

    /// Static tokens are shared, widely-copied credentials with no owning user.
    /// A `write` token must not become a skeleton key for restricted sets.
    #[test]
    fn anonymous_and_non_admin_static_tokens_are_public() {
        assert_eq!(RunVisibility::of(&anon()), RunVisibility::Public);
        assert_eq!(
            RunVisibility::of(&static_token(Scope::Read)),
            RunVisibility::Public
        );
        assert_eq!(
            RunVisibility::of(&static_token(Scope::Write)),
            RunVisibility::Public
        );
    }

    /// An admin API key belongs to a user, but the role wins: the check must
    /// not fall through to `User`, or an admin would be filtered by grants.
    #[test]
    fn the_admin_role_wins_over_the_user_arm() {
        let mut admin = account(Role::Admin, IdentitySource::ApiKey);
        admin.scope = Some(Scope::Admin);
        assert!(admin.user_id.is_some(), "the fixture must have a user id");
        assert_eq!(RunVisibility::of(&admin), RunVisibility::Full);
    }

    #[test]
    fn full_visibility_binds_nothing_and_is_always_true() {
        let mut args = Vec::new();
        let sql = visibility_predicate("runs", &RunVisibility::Full, &mut args);
        assert_eq!(sql, "1=1");
        assert!(args.is_empty());
    }

    #[test]
    fn public_visibility_checks_only_restrictions() {
        let mut args = Vec::new();
        let sql = visibility_predicate("r", &RunVisibility::Public, &mut args);
        assert!(args.is_empty(), "public binds no parameters");
        assert!(sql.starts_with("NOT EXISTS"));
        assert!(sql.contains("rsr.project = r.project"));
        assert!(sql.contains("rsr.suite IS NULL OR rsr.suite = r.suite"));
        assert!(!sql.contains("run_set_grants"));
    }

    /// The user id is bound, never interpolated, and numbered from wherever the
    /// caller's own parameters left off.
    #[test]
    fn user_visibility_binds_the_id_after_the_callers_parameters() {
        let mut args: Vec<Value> = vec![Value::Int(7), Value::Text("x".into())];
        let sql = visibility_predicate(
            "runs",
            &RunVisibility::User(UserId::new("usr_9")),
            &mut args,
        );
        assert_eq!(args.len(), 3);
        assert_eq!(args[2], Value::Text("usr_9".into()));
        assert!(sql.contains("g.user_id = ?3"), "{sql}");
        assert!(!sql.contains("usr_9"), "the id must never be interpolated");
        assert!(sql.contains("run_set_restrictions"));
        assert!(sql.contains("run_set_grants"));
    }

    /// Every arm has to be a legal SQL boolean over a `runs`-shaped table, or
    /// the first call site to append it fails at prepare time.
    #[test]
    fn every_arm_prepares_against_the_real_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE runs (id TEXT PRIMARY KEY, project TEXT, suite TEXT);
             CREATE TABLE run_set_restrictions (project TEXT NOT NULL, suite TEXT);
             CREATE TABLE run_set_grants (project TEXT NOT NULL, suite TEXT,
                                          user_id TEXT NOT NULL, level TEXT NOT NULL);",
        )
        .unwrap();
        for vis in [
            RunVisibility::Full,
            RunVisibility::Public,
            RunVisibility::User(UserId::new("usr_1")),
        ] {
            let mut args = Vec::new();
            let clause = visibility_predicate("runs", &vis, &mut args);
            let sql = format!("SELECT id FROM runs WHERE {clause}");
            conn.prepare(&sql)
                .unwrap_or_else(|e| panic!("{vis:?} produced unpreparable SQL: {e}\n{sql}"));
        }
    }
}
