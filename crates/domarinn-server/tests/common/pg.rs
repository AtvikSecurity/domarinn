//! Postgres test plumbing: one server per process, one database per test.
//!
//! `DOMARINN_TEST_BACKEND=postgres` switches the whole integration suite to
//! the Postgres backend. Where the server comes from:
//!
//! * `DOMARINN_TEST_DATABASE_URL` set — use it (CI and `mise run test-pg` do
//!   this so the ~30 test binaries share one server instead of each paying
//!   container startup).
//! * unset — start a throwaway `postgres:17-alpine` via testcontainers,
//!   shared by every test in this binary; the reaper removes it after the
//!   process exits.
//!
//! Each test gets its own `CREATE DATABASE` (`TEMPLATE template0 LOCALE 'C'`
//! — C collation matches SQLite's BINARY ordering, so `ORDER BY` over text
//! agrees across backends). Databases are not dropped: on a container they
//! die with it, and on a shared server they are `domarinn_test_*`-prefixed
//! and bounded by the CI job's lifetime.

use std::sync::OnceLock;

use testcontainers::runners::SyncRunner;
use testcontainers::ImageExt;

/// Whether this run targets the Postgres backend.
pub fn backend_is_postgres() -> bool {
    std::env::var("DOMARINN_TEST_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("postgres"))
}

// One instance per test process; size is irrelevant.
#[allow(clippy::large_enum_variant)]
enum PgServer {
    External(String),
    Container {
        url: String,
        // Held for the process's lifetime; dropping it would stop the
        // container while tests still run.
        _container: testcontainers::Container<testcontainers_modules::postgres::Postgres>,
    },
}

impl PgServer {
    fn admin_url(&self) -> &str {
        match self {
            PgServer::External(url) => url,
            PgServer::Container { url, .. } => url,
        }
    }
}

static SERVER: OnceLock<PgServer> = OnceLock::new();

fn server() -> &'static PgServer {
    SERVER.get_or_init(|| {
        if let Ok(url) = std::env::var("DOMARINN_TEST_DATABASE_URL") {
            return PgServer::External(url);
        }
        let container = testcontainers_modules::postgres::Postgres::default()
            .with_tag("17-alpine")
            // Parallel test threads each hold writer + reader connections
            // across two logical handles; the default 100 exhausts.
            .with_cmd(["postgres", "-c", "max_connections=500"])
            .start()
            .expect("start postgres testcontainer (is Docker running?)");
        let port = container
            .get_host_port_ipv4(5432)
            .expect("postgres container port");
        PgServer::Container {
            url: format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres"),
            _container: container,
        }
    })
}

/// Create a fresh database and return its connection URL. Blocking — callers
/// on a runtime wrap it in `spawn_blocking`.
pub fn fresh_database_url() -> String {
    // pid + per-process counter, not a timestamp: parallel test threads (and
    // the ~30 test binaries sharing one server) sample identical nanoseconds
    // often enough to collide on CREATE DATABASE.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let admin_url = server().admin_url().to_owned();
    let mut client =
        postgres::Client::connect(&admin_url, postgres::NoTls).expect("connect to test postgres");
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The millis suffix guards the one remaining collision: a later test
    // binary recycling an earlier one's pid against the same shared server.
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let name = format!("domarinn_test_{}_{seq}_{millis}", std::process::id());
    client
        .batch_execute(&format!(
            "CREATE DATABASE {name} TEMPLATE template0 LOCALE 'C'"
        ))
        .expect("create test database");
    let (base, _) = admin_url.rsplit_once('/').expect("url has a path");
    format!("{base}/{name}")
}
