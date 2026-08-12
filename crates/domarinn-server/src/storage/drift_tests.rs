//! The schema-drift guard: the only thing standing between `schema.rs`'s
//! migration lineage and `schema_pg.rs`'s consolidated snapshot silently
//! diverging. Every new SQLite migration must be mirrored as a new
//! `schema_pg.rs` version; this test fails when someone forgets.
//!
//! Needs a reachable Postgres (`DOMARINN_TEST_DATABASE_URL`); skips
//! otherwise, and the `test-pg` CI job always provides one.

use std::collections::{BTreeMap, BTreeSet};

use super::{pg, schema};

/// Columns that exist only on the Postgres side, by design: the identity id
/// replacing SQLite's implicit rowid, and the generated tsvector columns on
/// the FTS mirror tables.
const PG_ONLY_COLUMNS: &[(&str, &[&str])] = &[
    ("cache_entries", &["id"]),
    ("cache_entries_fts", &["id", "tsv"]),
    ("runs_fts", &["tsv"]),
    ("cases_fts", &["tsv"]),
];

/// The migration ledgers themselves, which have no SQLite counterpart
/// (rusqlite_migration tracks versions in `user_version`).
const PG_ONLY_TABLES: &[&str] = &["schema_migrations", "cache_schema_migrations"];

fn sqlite_tables() -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    let mut out = BTreeMap::new();
    for cache in [false, true] {
        let mut conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        if cache {
            schema::cache_migrations().to_latest(&mut conn)?;
        } else {
            schema::migrate_runs(&mut conn)?;
        }
        let names: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )?;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            names
        };
        // FTS5 virtual tables bring shadow tables (`runs_fts_data`, …);
        // those are implementation details of SQLite's FTS engine, not
        // schema to mirror.
        let fts: Vec<String> = names
            .iter()
            .filter(|n| n.ends_with("_fts"))
            .cloned()
            .collect();
        for name in names {
            if fts.iter().any(|f| name.starts_with(&format!("{f}_"))) {
                continue;
            }
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({name})"))?;
            let cols: BTreeSet<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<Result<_, _>>()?;
            out.insert(name.clone(), cols);
        }
    }
    Ok(out)
}

fn pg_tables(url: &str) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    let connector = pg::PgConnector::new(url)?;
    let mut client = connector.connect()?;
    pg::migrate(&mut client)?;
    let rows = client.query(
        "SELECT table_name, column_name FROM information_schema.columns
         WHERE table_schema = 'public'
         ORDER BY table_name, column_name",
        &[],
    )?;
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        out.entry(row.get(0)).or_default().insert(row.get(1));
    }
    for table in PG_ONLY_TABLES {
        out.remove(*table);
    }
    Ok(out)
}

#[test]
fn postgres_schema_mirrors_sqlite_final_state() {
    let Ok(admin_url) = std::env::var("DOMARINN_TEST_DATABASE_URL") else {
        eprintln!("DOMARINN_TEST_DATABASE_URL unset; skipping schema drift check");
        return;
    };
    // Own database so a concurrently-running integration suite never
    // half-migrates under this test.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let name = format!("domarinn_drift_{nanos}");
    let mut admin = postgres::Client::connect(&admin_url, postgres::NoTls).expect("admin connect");
    admin
        .batch_execute(&format!(
            "CREATE DATABASE {name} TEMPLATE template0 LOCALE 'C'"
        ))
        .expect("create drift database");
    let (base, _) = admin_url.rsplit_once('/').expect("url path");
    let url = format!("{base}/{name}");

    let sqlite = sqlite_tables().expect("sqlite schema");
    let mut postgres = pg_tables(&url).expect("postgres schema");

    for (table, extras) in PG_ONLY_COLUMNS {
        if let Some(cols) = postgres.get_mut(*table) {
            for extra in *extras {
                assert!(
                    cols.remove(*extra),
                    "expected PG-only column {table}.{extra} is missing"
                );
            }
        }
    }

    let sqlite_names: BTreeSet<&String> = sqlite.keys().collect();
    let pg_names: BTreeSet<&String> = postgres.keys().collect();
    assert_eq!(
        sqlite_names, pg_names,
        "table sets diverge — mirror the new/renamed table into schema_pg.rs"
    );
    for (table, cols) in &sqlite {
        assert_eq!(
            cols, &postgres[table],
            "column set of `{table}` diverges — mirror the migration into schema_pg.rs"
        );
    }

    let _ = admin.batch_execute(&format!("DROP DATABASE {name}"));
}
