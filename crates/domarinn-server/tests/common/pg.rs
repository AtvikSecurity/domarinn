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
//! Each test gets its own `CREATE DATABASE`, cloned from a **template
//! database** this process migrates exactly once — a clone is a file-level
//! copy, so the per-test cost is milliseconds instead of replaying both
//! migration ledgers, and many tests can create databases at once. The
//! template is created `TEMPLATE template0 LOCALE 'C'` (C collation matches
//! SQLite's BINARY ordering, so `ORDER BY` over text agrees across
//! backends); clones inherit the locale. Databases are not dropped: on a
//! container they die with it, and on a shared server they are
//! `domarinn_test_*`-prefixed and bounded by the CI job's lifetime.

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

/// A unique database name: pid + per-process counter, not a timestamp —
/// parallel test threads (and the ~30 test binaries sharing one server)
/// sample identical nanoseconds often enough to collide on CREATE DATABASE.
/// The millis suffix guards the one remaining collision: a later test binary
/// recycling an earlier one's pid against the same shared server.
fn unique_name(prefix: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    format!("{prefix}_{}_{seq}_{millis}", std::process::id())
}

fn database_url(admin_url: &str, name: &str) -> String {
    let (base, _) = admin_url.rsplit_once('/').expect("url has a path");
    format!("{base}/{name}")
}

static TEMPLATE: OnceLock<String> = OnceLock::new();

/// The once-per-process template: created empty, migrated through the
/// public `Storage::open_postgres` path, then left connection-free so
/// clones can use it as a `TEMPLATE`.
fn template_name(admin_url: &str) -> &'static str {
    TEMPLATE.get_or_init(|| {
        let name = unique_name("domarinn_tmpl");
        let mut admin = postgres::Client::connect(admin_url, postgres::NoTls)
            .expect("connect to test postgres");
        admin
            .batch_execute(&format!(
                "CREATE DATABASE {name} TEMPLATE template0 LOCALE 'C'"
            ))
            .expect("create template database");
        // Migrate via the public open path. We are on a plain (blocking)
        // thread here, so a private runtime may drive the async open; the
        // sync postgres clients inside are created and dropped off any
        // caller runtime, which is the one rule they care about.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("template runtime");
        let url = database_url(admin_url, &name);
        let storage = rt
            .block_on(domarinn_server::storage::Storage::open_postgres(url))
            .expect("migrate template database");
        drop(storage);
        drop(rt);
        // `CREATE DATABASE ... TEMPLATE` requires the template to have no
        // connections, and Storage's drop closes its clients on a detached
        // thread — wait for the server to agree they are gone.
        for _ in 0..200 {
            let open: i64 = admin
                .query_one(
                    "SELECT COUNT(*) FROM pg_stat_activity WHERE datname = $1",
                    &[&name],
                )
                .expect("pg_stat_activity")
                .get(0);
            if open == 0 {
                return name;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("template database {name} still has open connections");
    })
}

/// Create a fresh database (cloned from the migrated template) and return
/// its connection URL. Blocking — callers on a runtime wrap it in
/// `spawn_blocking`.
pub fn fresh_database_url() -> String {
    let admin_url = server().admin_url().to_owned();
    let template = template_name(&admin_url);
    let mut client =
        postgres::Client::connect(&admin_url, postgres::NoTls).expect("connect to test postgres");
    let name = unique_name("domarinn_test");
    // Cloning can transiently fail while another clone of the same template
    // is mid-flight ("source database is being accessed"); retry briefly
    // rather than serializing every test through a lock.
    for attempt in 0.. {
        match client.batch_execute(&format!("CREATE DATABASE {name} TEMPLATE {template}")) {
            Ok(()) => break,
            Err(e) if attempt < 100 && e.to_string().contains("is being accessed") => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("create test database from template: {e}"),
        }
    }
    database_url(&admin_url, &name)
}
