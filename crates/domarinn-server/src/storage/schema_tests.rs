//! Unit tests for [`super`] (the migrations). Split out of `schema.rs` via
//! `#[path]` to keep that file under the repo's 1000-line source cap;
//! this is still the schema module's private child (`use super::*`).

use super::*;
use rusqlite::{params, Connection};

/// A runs database stopped at migration 13, with foreign keys enforced
/// exactly as `open_conn` configures the real writer connection — the
/// state every deployed instance upgrades into migration 14 from.
fn v13_conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    runs_migrations().to_version(&mut conn, 13).unwrap();
    conn
}

/// A runs database at the latest migration, reached through the same
/// [`migrate_runs`] the server's own open path uses.
fn latest_conn() -> Connection {
    let mut conn = v13_conn();
    migrate_runs(&mut conn).unwrap();
    conn
}

fn seed_user(conn: &Connection, id: &str, username: &str, role: &str) {
    conn.execute(
        "INSERT INTO users (id, username, password_hash, role, disabled, created_at, email)
         VALUES (?1, ?2, 'phc', ?3, 0, 1700000000000, ?4)",
        params![id, username, role, format!("{username}@example.com")],
    )
    .unwrap();
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Migration 19 rebuilds `cases` to widen its status CHECK (the xfail/
/// xpass statuses). The rebuild must not lose rows or their `case_tags`
/// children, must recreate every index, and afterwards the CHECK admits
/// the two new statuses while still rejecting garbage.
#[test]
fn migration_19_widens_the_case_status_check_without_losing_rows() {
    let mut conn = v13_conn();
    conn.execute_batch(
        "INSERT INTO runs (id, project, created_at, uploaded_at, schema_version, case_count,
                           pass_count, fail_count, error_count, prompt_tokens,
                           completion_tokens, duration_ms, content_hash)
             VALUES ('r1', 'p', 1, 1, 3, 1, 1, 0, 0, 0, 0, 0, 'h');
         INSERT INTO cases (run_id, case_key, idx, name, status, provider_id, test_id, score,
                            cache_key)
             VALUES ('r1', 'ck1', 0, 'greet', 'pass', 'prov', 'greet', 1.0, 'sha256:aa');
         INSERT INTO case_tags (run_id, case_key, tag) VALUES ('r1', 'ck1', 'smoke');",
    )
    .unwrap();

    migrate_runs(&mut conn).unwrap();

    // The row and its child survive, values intact.
    let (name, status, provider, cache_key): (String, String, String, String) = conn
        .query_row(
            "SELECT name, status, provider_id, cache_key FROM cases WHERE case_key = 'ck1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (
            name.as_str(),
            status.as_str(),
            provider.as_str(),
            cache_key.as_str()
        ),
        ("greet", "pass", "prov", "sha256:aa")
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM case_tags"), 1);

    // The widened CHECK admits the new statuses and rejects garbage.
    for status in ["xfail", "xpass"] {
        conn.execute(
            "INSERT INTO cases (run_id, case_key, idx, status)
                 VALUES ('r1', ?1, 1, ?2)",
            params![format!("ck-{status}"), status],
        )
        .unwrap();
    }
    assert!(conn
        .execute(
            "INSERT INTO cases (run_id, case_key, idx, status)
                 VALUES ('r1', 'ck-bad', 9, 'bogus')",
            [],
        )
        .is_err());

    // Every `cases` index — the partial `idx_cases_cache_key` included —
    // exists after the rebuild.
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='cases'")
        .unwrap();
    let indexes: std::collections::HashSet<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for expected in [
        "idx_cases_run_status",
        "idx_cases_run_provider",
        "idx_cases_run_test",
        "idx_cases_key",
        "idx_cases_run_error_class",
        "idx_cases_cache_key",
        "idx_cases_run_empty_reason",
    ] {
        assert!(
            indexes.contains(expected),
            "missing {expected}: {indexes:?}"
        );
    }

    // The run-level counters exist and read 0-ish (NULL) on old rows.
    let (xf, xp): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT xfail_count, xpass_count FROM runs WHERE id = 'r1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((xf, xp), (None, None), "no backfill: NULL honestly reads 0");
}

/// The `users` rebuild is the risky half of migration 14: SQLite cannot
/// widen a CHECK in place, and a naive rebuild drops every session, API
/// key and SSO identity on the floor via `ON DELETE CASCADE`.
#[test]
fn migration_14_rebuilds_users_without_losing_rows_or_their_children() {
    let mut conn = v13_conn();
    seed_user(&conn, "u1", "root", "admin");
    seed_user(&conn, "u2", "dana", "member");
    conn.execute_batch(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_used_at)
             VALUES ('sh', 'u2', 1, 2, 3);
         INSERT INTO api_keys (id, user_id, name, prefix, key_hash, scope, created_at)
             VALUES ('k1', 'u2', 'ci', 'domarinn_ab', 'kh', 'write', 1);
         INSERT INTO user_identities
             (id, user_id, provider, kind, subject, email, display_name, created_at)
             VALUES ('i1', 'u2', 'oidc:corp', 'oidc', 'sub-1', 'd@example.com', 'Dana', 1);",
    )
    .unwrap();

    migrate_runs(&mut conn).unwrap();

    // Every user column survives the rebuild, values included.
    let (username, role, email, disabled, created_at): (String, String, String, i64, i64) = conn
        .query_row(
            "SELECT username, role, email, disabled, created_at FROM users WHERE id = 'u2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(username, "dana");
    assert_eq!(role, "member");
    assert_eq!(email, "dana@example.com");
    assert_eq!(disabled, 0);
    assert_eq!(created_at, 1_700_000_000_000);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM users"), 2);

    // The children the FKs point at are still there.
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM sessions"), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM api_keys"), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM user_identities"), 1);

    // `username` is still unique.
    assert!(conn
        .execute(
            "INSERT INTO users (id, username, password_hash, role, disabled, created_at)
             VALUES ('u3', 'dana', 'phc', 'member', 0, 1)",
            [],
        )
        .is_err());

    // And the cascade still fires, so the FKs point at the new table.
    conn.execute("DELETE FROM users WHERE id = 'u2'", [])
        .unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM sessions"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM api_keys"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM user_identities"), 0);
}

#[test]
fn migration_14_admits_viewer_and_still_rejects_unknown_roles() {
    let conn = latest_conn();
    seed_user(&conn, "u1", "reader", "viewer");
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM users WHERE role = 'viewer'"),
        1
    );
    assert!(conn
        .execute(
            "INSERT INTO users (id, username, password_hash, role, disabled, created_at)
             VALUES ('u2', 'sneaky', 'phc', 'superadmin', 0, 1)",
            [],
        )
        .is_err());
}

/// A project-level restriction is one row, not many: plain `UNIQUE` would
/// treat every NULL suite as distinct and let duplicates through.
#[test]
fn a_project_level_restriction_can_only_be_recorded_once() {
    let conn = latest_conn();
    let insert = |suite: Option<&str>| {
        conn.execute(
            "INSERT INTO run_set_restrictions (project, suite, created_at, created_by)
             VALUES ('checkout', ?1, 1, 'root')",
            params![suite],
        )
    };
    assert!(insert(None).is_ok());
    assert!(insert(None).is_err(), "NULL suites must collide");
    assert!(insert(Some("smoke")).is_ok());
    assert!(insert(Some("smoke")).is_err());
}

#[test]
fn a_grant_is_unique_per_scope_and_dies_with_its_user() {
    let conn = latest_conn();
    seed_user(&conn, "u1", "reader", "viewer");
    let insert = |id: &str, suite: Option<&str>, level: &str| {
        conn.execute(
            "INSERT INTO run_set_grants
                 (id, project, suite, user_id, level, created_at, created_by)
             VALUES (?1, 'checkout', ?2, 'u1', ?3, 1, 'root')",
            params![id, suite, level],
        )
    };
    assert!(insert("g1", None, "view").is_ok());
    assert!(
        insert("g2", None, "manage").is_err(),
        "one grant per project/suite/user, whatever its level"
    );
    assert!(insert("g3", Some("smoke"), "upload").is_ok());
    assert!(insert("g4", Some("smoke"), "upload").is_err());
    assert!(
        insert("g5", Some("other"), "owner").is_err(),
        "unknown level"
    );

    conn.execute("DELETE FROM users WHERE id = 'u1'", [])
        .unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM run_set_grants"), 0);
}
