//! Run-set policy: which `(project, suite)` pairs are restricted, and who
//! holds a grant on them.
//!
//! The tables (migration 14) live in the runs database, so this module shares
//! its writer mutex and reader pool like every other storage submodule. The
//! access *model* — levels, identity classes, and the SQL predicate every run
//! query appends — lives in [`crate::runsets`]; this module only stores and
//! answers questions about it.
//!
//! # Scope matching
//!
//! A row with a NULL `suite` covers the whole project; a row with a suite
//! covers just that suite. Both the restriction check and the grant lookup use
//! the same `(suite IS NULL OR suite = ?)` shape, which also gives the right
//! answer for a run with no suite: `suite = NULL` is NULL, never true, so only
//! the project-level row covers it.
//!
//! A run with no *project* can never be covered at all (`project = NULL` is
//! likewise never true), which is why such runs are always accessible.

use rusqlite::{params, Connection, OptionalExtension};

use super::{now_ms, Storage};
use crate::domain::UserId;
use crate::runsets::{GrantLevel, RunVisibility};

/// One grant, joined with the username it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSetGrant {
    pub id: String,
    pub project: String,
    pub suite: Option<String>,
    pub user_id: UserId,
    pub username: String,
    pub level: GrantLevel,
    pub created_at: i64,
    pub created_by: Option<String>,
}

impl Storage {
    /// Mark `(project, suite)` restricted. `suite: None` restricts the whole
    /// project. Returns `true` when this created the restriction, `false` when
    /// it already existed (idempotent).
    pub async fn restrict_run_set(
        &self,
        project: String,
        suite: Option<String>,
        created_by: Option<String>,
    ) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                // The uniqueness index is on `(project, COALESCE(suite,''))`, so
                // the conflict target has to name the same expression.
                let n = conn.execute(
                    "INSERT INTO run_set_restrictions (project, suite, created_at, created_by)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(project, COALESCE(suite,'')) DO NOTHING",
                    params![project, suite, now_ms(), created_by],
                )?;
                Ok(n > 0)
            })
            .await
    }

    /// Drop the restriction on `(project, suite)`. Returns `true` when a row
    /// was removed. Grants are left in place: a set that is restricted again
    /// keeps the access list it had.
    pub async fn unrestrict_run_set(
        &self,
        project: String,
        suite: Option<String>,
    ) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n = conn.execute(
                    "DELETE FROM run_set_restrictions
                      WHERE project = ?1 AND COALESCE(suite,'') = COALESCE(?2,'')",
                    params![project, suite],
                )?;
                Ok(n > 0)
            })
            .await
    }

    /// Whether `(project, suite)` is covered by a restriction — either its own
    /// suite-level row or a project-level one.
    pub async fn run_set_restricted(
        &self,
        project: Option<String>,
        suite: Option<String>,
    ) -> anyhow::Result<bool> {
        self.runs
            .read(move |conn| restricted(conn, project.as_deref(), suite.as_deref()))
            .await
    }

    /// Whether `(project, suite)` carries a restriction row of *its own* scope.
    ///
    /// The exact-scope counterpart of [`Storage::run_set_restricted`], and the
    /// question the access editor asks: this is the row
    /// [`Storage::restrict_run_set`] writes and [`Storage::unrestrict_run_set`]
    /// removes, so a suite inside a restricted project answers `false` — its
    /// restriction lives on the project, and that is where it is lifted.
    /// Exact scope for the same reason [`Storage::list_run_set_grants`] is.
    pub async fn run_set_restricted_exactly(
        &self,
        project: String,
        suite: Option<String>,
    ) -> anyhow::Result<bool> {
        self.runs
            .read(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT 1 FROM run_set_restrictions
                          WHERE project = ?1 AND COALESCE(suite,'') = COALESCE(?2,'') LIMIT 1",
                        params![project, suite],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some())
            })
            .await
    }

    /// Create or re-level a user's grant on `(project, suite)`.
    pub async fn upsert_run_set_grant(
        &self,
        project: String,
        suite: Option<String>,
        user_id: UserId,
        level: GrantLevel,
        created_by: Option<String>,
    ) -> anyhow::Result<()> {
        self.runs
            .write(move |conn| {
                let id = ulid::Ulid::generate().to_string();
                conn.execute(
                    "INSERT INTO run_set_grants
                         (id, project, suite, user_id, level, created_at, created_by)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(project, COALESCE(suite,''), user_id)
                     DO UPDATE SET level = excluded.level",
                    params![id, project, suite, user_id, level, now_ms(), created_by],
                )?;
                Ok(())
            })
            .await
    }

    /// Remove a user's grant on exactly `(project, suite)`. Returns `true` when
    /// a row was removed.
    pub async fn delete_run_set_grant(
        &self,
        project: String,
        suite: Option<String>,
        user_id: UserId,
    ) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n = conn.execute(
                    "DELETE FROM run_set_grants
                      WHERE project = ?1 AND COALESCE(suite,'') = COALESCE(?2,'') AND user_id = ?3",
                    params![project, suite, user_id],
                )?;
                Ok(n > 0)
            })
            .await
    }

    /// Every grant recorded on exactly `(project, suite)`, oldest first.
    ///
    /// Exact scope, not covering scope: this answers "who is on this set's
    /// access list", which is what an editor shows. Whether a *caller* may act
    /// is [`Storage::set_access`]'s question, and that one does cover.
    pub async fn list_run_set_grants(
        &self,
        project: String,
        suite: Option<String>,
    ) -> anyhow::Result<Vec<RunSetGrant>> {
        self.runs
            .read(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT g.id, g.project, g.suite, g.user_id, u.username, g.level,
                            g.created_at, g.created_by
                       FROM run_set_grants g JOIN users u ON u.id = g.user_id
                      WHERE g.project = ?1 AND COALESCE(g.suite,'') = COALESCE(?2,'')
                      ORDER BY g.created_at ASC, u.username ASC",
                )?;
                let rows = stmt.query_map(params![project, suite], |row| {
                    Ok(RunSetGrant {
                        id: row.get(0)?,
                        project: row.get(1)?,
                        suite: row.get(2)?,
                        user_id: row.get(3)?,
                        username: row.get(4)?,
                        level: row.get(5)?,
                        created_at: row.get(6)?,
                        created_by: row.get(7)?,
                    })
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
    }

    /// The strongest level `user_id` holds over `(project, suite)`, counting a
    /// project-level grant as covering every suite in it.
    pub async fn run_set_grant_level(
        &self,
        project: Option<String>,
        suite: Option<String>,
        user_id: UserId,
    ) -> anyhow::Result<Option<GrantLevel>> {
        self.runs
            .read(move |conn| covering_level(conn, project.as_deref(), suite.as_deref(), &user_id))
            .await
    }

    /// Whether this caller may act on `(project, suite)` at `needed`.
    ///
    /// The single-resource counterpart of
    /// [`crate::runsets::visibility_predicate`], used by the write paths.
    ///
    /// * [`RunVisibility::Full`] — always.
    /// * Anyone else — an explicit covering grant of at least `needed`, **or**
    ///   the default-open waiver below.
    ///
    /// # The default-open waiver stops at `upload`
    ///
    /// On an *unrestricted* set, `View` and `Upload` are free to every class:
    /// that is what default-open means, and it is why an anonymous upload in
    /// `open` mode still works exactly as it did before run sets existed.
    ///
    /// [`GrantLevel::Manage`] is deliberately outside the waiver. Managing a
    /// set is the power to restrict it and to hand out grants on it — including
    /// to yourself — so a level-blind waiver would mean any caller who can
    /// reach a write endpoint could seize a project that nobody had restricted
    /// yet, which is every project on a fresh instance. Bootstrapping a
    /// restriction is therefore admin-only, and afterwards it takes a covering
    /// `manage` grant. Such a grant is honoured whether or not the set is
    /// restricted, so it may sit dormant on an open set and still count.
    pub async fn set_access(
        &self,
        vis: RunVisibility,
        project: Option<String>,
        suite: Option<String>,
        needed: GrantLevel,
    ) -> anyhow::Result<bool> {
        if vis == RunVisibility::Full {
            return Ok(true);
        }
        self.runs
            .read(move |conn| {
                if needed <= GrantLevel::Upload
                    && !restricted(conn, project.as_deref(), suite.as_deref())?
                {
                    return Ok(true);
                }
                let RunVisibility::User(user_id) = &vis else {
                    return Ok(false);
                };
                let held = covering_level(conn, project.as_deref(), suite.as_deref(), user_id)?;
                Ok(held.is_some_and(|level| level >= needed))
            })
            .await
    }
}

pub(super) fn restricted(
    conn: &Connection,
    project: Option<&str>,
    suite: Option<&str>,
) -> anyhow::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM run_set_restrictions
              WHERE project = ?1 AND (suite IS NULL OR suite = ?2) LIMIT 1",
            params![project, suite],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The strongest of the (at most two) grants that cover `(project, suite)`.
/// Ordered in Rust rather than SQL: the stored levels are words, and `MAX()`
/// over them would order them alphabetically — `manage` < `upload` < `view` —
/// i.e. exactly backwards.
pub(super) fn covering_level(
    conn: &Connection,
    project: Option<&str>,
    suite: Option<&str>,
    user_id: &UserId,
) -> anyhow::Result<Option<GrantLevel>> {
    let mut stmt = conn.prepare(
        "SELECT level FROM run_set_grants
          WHERE user_id = ?3 AND project = ?1 AND (suite IS NULL OR suite = ?2)",
    )?;
    let rows = stmt.query_map(params![project, suite, user_id], |row| {
        row.get::<_, GrantLevel>(0)
    })?;
    let mut best: Option<GrantLevel> = None;
    for level in rows {
        let level = level?;
        best = Some(best.map_or(level, |b: GrantLevel| b.max(level)));
    }
    Ok(best)
}
