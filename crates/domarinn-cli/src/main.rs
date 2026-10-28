//! domarinn — the CLI entry point.
//!
//! Exit codes: `0` all pass; `1` assertion failures / regression; `2`
//! config/usage error; `3` infrastructure error. `3` wins over `1` so CI can
//! distinguish "the model got worse" from "the harness broke".

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand};

mod baseline;
mod baselinecmd;
mod cachecfg;
mod cachecmd;
mod cachels;
mod casedetail;
mod cisummary;
mod diffcmd;
mod diffmd;
mod diffrender;
mod errorstats;
mod import;
mod loadrun;
mod output;
mod outputmd;
mod outputplan;
mod progress;
mod run;
mod runscmd;
mod share;
mod style;

/// Exit codes with meaning to CI.
pub mod exit {
    pub const OK: u8 = 0;
    pub const ASSERT_FAIL: u8 = 1;
    pub const USAGE: u8 = 2;
    pub const INFRA: u8 = 3;
}

/// Report a final-outcome failure holding an [`anyhow::Error`] and return its
/// exit code. `{err:#}` prints the whole `anyhow` context chain — plain
/// `Display` shows only the outermost context (e.g. "opening sqlite db"),
/// hiding the root cause (e.g. the underlying permissions error). Routing every
/// `anyhow` outcome site through here keeps that chain visible and prevents a
/// future `error: {e}` from silently dropping it.
fn fail(code: u8, err: &anyhow::Error) -> u8 {
    eprintln!("error: {err:#}");
    code
}

#[derive(Parser)]
#[command(name = "domarinn", version, about = "A declarative LLM eval harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Increase logging verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Log format: auto, pretty, compact, json (env: DOMARINN_LOG_FORMAT).
    /// Logs go to stderr; RUST_LOG overrides the default filter entirely.
    #[arg(long, global = true, value_enum, default_value_t = LogFormatArg::Auto)]
    log_format: LogFormatArg,

    /// Results server base URL (or set DOMARINN_SERVER_URL).
    #[arg(long, global = true)]
    server_url: Option<String>,

    /// When to colorize human output: auto (color a TTY), always, or never.
    /// Honors NO_COLOR and CLICOLOR_FORCE; machine formats are never colored.
    #[arg(long, global = true, value_enum, default_value_t = style::ColorChoice::Auto)]
    color: style::ColorChoice,
}

#[derive(Subcommand)]
enum Command {
    /// Run a suite: call providers, evaluate assertions, report results.
    // Boxed: `RunArgs` is by far the widest variant, so inlining it makes every
    // `Command` — including `cache path` — as large as the run parser.
    Run(Box<run::RunArgs>),
    /// Upload a completed run to a results server and print its URL.
    Share(share::ShareArgs),
    /// Summarize a run for CI: a markdown report plus GitHub Actions outputs.
    CiSummary(cisummary::CiSummaryArgs),
    /// List stored runs (newest first).
    Runs(runscmd::RunsArgs),
    /// Manage the server-side baseline pin for a suite (show/set/clear).
    Baseline(baselinecmd::BaselineArgs),
    /// Diff two runs (regressions, fixes, output changes, significance).
    Diff(diffcmd::DiffArgs),
    /// Render a stored run in the terminal.
    View(diffcmd::ViewArgs),
    /// Manage the local response cache.
    ///
    /// These commands operate on the local disk tier only, whichever backend a
    /// suite configures. A shared remote is pruned server-side (`POST
    /// /api/v1/cache/prune`) or by the bucket's own lifecycle rules.
    Cache {
        #[command(subcommand)]
        cmd: cachecmd::CacheCmd,
    },
    /// Import a config from another tool into a domarinn suite.
    Import {
        /// The tool whose config PATH is written for.
        #[arg(value_enum)]
        format: ImportFormat,
        /// The config file to translate (e.g. promptfooconfig.yaml). The suite
        /// is printed to stdout.
        path: PathBuf,
    },
    /// Generate TypeScript type definitions for the result/diff DTOs.
    GenTypes {
        /// Directory to write the .ts files into (created if missing).
        #[arg(default_value = "web/src/api/generated")]
        dir: PathBuf,
    },
    /// Parse and structurally validate a suite (no provider calls).
    Validate {
        /// Path to a suite file or a directory containing domarinn.yaml.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Print a JSON Schema (for editor completion / CI contract).
    Schema {
        /// `config` is the suite (domarinn.yaml) schema; `result` is the
        /// RunResult document schema.
        #[arg(value_enum)]
        which: SchemaKind,
    },
    /// List the tests, providers, or prompts a suite resolves to.
    List {
        #[arg(value_enum)]
        what: ListKind,
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Run the suite's test generators so their cases are listed too.
        /// Off by default: generators are subprocesses, and `list` is
        /// otherwise a read-only command.
        #[arg(long)]
        generators: bool,
    },
    /// Run the self-hostable results server + web UI.
    ///
    /// Stops gracefully on SIGTERM or Ctrl-C: in-flight requests finish before
    /// the process exits.
    Server {
        /// Listen port (env: DOMARINN_PORT).
        #[arg(long, env = "DOMARINN_PORT", default_value_t = 8321)]
        port: u16,
        /// Listen address, IPv4 or IPv6 (env: DOMARINN_BIND). The default,
        /// 0.0.0.0, is every IPv4 interface — what a container needs; use
        /// 127.0.0.1 or ::1 to keep a bare-metal server off the network.
        #[arg(long, env = "DOMARINN_BIND", default_value_t = domarinn_server::DEFAULT_BIND)]
        bind: IpAddr,
        /// State directory holding the SQLite databases (env: DOMARINN_DATA_DIR).
        #[arg(long, env = "DOMARINN_DATA_DIR", default_value = "/data")]
        data_dir: PathBuf,
    },
    /// Copy an existing SQLite deployment into an empty Postgres database.
    ///
    /// Run it with the server stopped; afterwards start the server with
    /// DOMARINN_DATABASE_URL pointing at the same database. The target must
    /// be empty — the tool refuses to merge into live data.
    MigrateDb {
        /// State directory holding the SQLite databases (env: DOMARINN_DATA_DIR).
        #[arg(long, env = "DOMARINN_DATA_DIR", default_value = "/data")]
        data_dir: PathBuf,
        /// Postgres connection URL to copy into (env: DOMARINN_DATABASE_URL).
        #[arg(long, env = "DOMARINN_DATABASE_URL")]
        database_url: String,
    },
    /// Probe this binary's own /api/v1/health (used by container HEALTHCHECK).
    ///
    /// Sends an HTTP GET and exits 0 on a 2xx response, 3 otherwise (refused,
    /// timed out after 3s, or a non-2xx status).
    Healthcheck {
        /// Port the server listens on (env: DOMARINN_PORT — the same variable
        /// `server` reads, so one setting moves both).
        #[arg(long, env = "DOMARINN_PORT", default_value_t = 8321)]
        port: u16,
        /// Host or IP to probe. Defaults to loopback, or to DOMARINN_BIND when
        /// that names a specific address (a server bound to one interface is
        /// not reachable on loopback).
        #[arg(long)]
        host: Option<String>,
    },
    /// Print a shell completion script to stdout.
    ///
    /// For example: `domarinn completions bash > ~/.local/share/bash-completion/completions/domarinn`,
    /// or `domarinn completions zsh > "${fpath[1]}/_domarinn"`.
    Completions {
        /// The shell to generate completions for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum SchemaKind {
    Config,
    Result,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum ListKind {
    Tests,
    Providers,
    Prompts,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum ImportFormat {
    Promptfoo,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum LogFormatArg {
    Auto,
    Pretty,
    Compact,
    Json,
}

impl From<LogFormatArg> for domarinn_logging::LogFormat {
    fn from(value: LogFormatArg) -> Self {
        match value {
            LogFormatArg::Auto => domarinn_logging::LogFormat::Auto,
            LogFormatArg::Pretty => domarinn_logging::LogFormat::Pretty,
            LogFormatArg::Compact => domarinn_logging::LogFormat::Compact,
            LogFormatArg::Json => domarinn_logging::LogFormat::Json,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let profile = match &cli.command {
        Command::Server { .. } => domarinn_logging::LogProfile::Server,
        _ => domarinn_logging::LogProfile::Cli,
    };
    domarinn_logging::init(&domarinn_logging::LogOptions {
        profile,
        verbose: cli.verbose,
        format: cli.log_format.into(),
    });

    // Resolve the color decision once, from the flag + environment + stdout TTY,
    // and thread the same palette through every command.
    let palette = style::Palette::detect(cli.color);

    let code = match cli.command {
        Command::Run(args) => run::execute(*args, cli.server_url, palette, cli.verbose),
        Command::Share(args) => share::execute(args, cli.server_url),
        Command::CiSummary(args) => cisummary::execute(args, cli.server_url),
        Command::Runs(args) => runscmd::execute(args, cli.server_url, palette),
        Command::Baseline(args) => baselinecmd::execute(args, cli.server_url),
        Command::Diff(args) => diffcmd::execute_diff(args, palette),
        Command::View(args) => diffcmd::execute_view(args, palette),
        Command::Cache { cmd } => cachecmd::execute(cmd),
        Command::Import { format, path } => match format {
            ImportFormat::Promptfoo => import::execute(path),
        },
        Command::GenTypes { dir } => cmd_gen_types(&dir),
        Command::Validate { path } => cmd_validate(&path, palette),
        Command::Schema { which } => cmd_schema(which),
        Command::List {
            what,
            path,
            json,
            generators,
        } => cmd_list(what, &path, json, generators),
        Command::Server {
            port,
            bind,
            data_dir,
        } => cmd_server(port, bind, data_dir),
        Command::MigrateDb {
            data_dir,
            database_url,
        } => cmd_migrate_db(data_dir, database_url),
        Command::Healthcheck { port, host } => cmd_healthcheck(port, host),
        Command::Completions { shell } => cmd_completions(shell),
    };
    ExitCode::from(code)
}

/// Structural validation only. Warnings print but never change the exit code —
/// exit `2` is documented as a config/usage error, and a shape that is merely
/// *probably* wrong (an Anthropic assistant prefill is a real one) must stay
/// runnable in CI.
fn cmd_validate(path: &Path, palette: style::Palette) -> u8 {
    let (suite, raw) = match domarinn_core::loader::load_file_raw(path) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{} {e}", palette.fail("error:"));
            return exit::USAGE;
        }
    };
    let report = domarinn_core::validate(&suite, &raw);
    let errors = report.errors().count();
    let warnings = report.warnings().count();

    if errors + warnings > 0 {
        // Header keeps its exact pre-warnings wording when everything is an
        // error, so the existing CLI tests and anyone grepping it are unmoved.
        let header = if warnings == 0 {
            format!("{} validation issue(s):", errors)
        } else {
            format!(
                "{} validation issue(s) ({errors} error(s), {warnings} warning(s)):",
                errors + warnings
            )
        };
        eprintln!("{header}");
        // Every line is labelled, always — a line whose shape depends on
        // whether some *other* finding exists is miserable to read and to test.
        for issue in report.issues() {
            let label = match issue.severity {
                domarinn_core::Severity::Error => palette.fail("error:"),
                domarinn_core::Severity::Warning => palette.warn("warning:"),
            };
            eprintln!("  - {label} {issue}");
        }
    }

    if errors > 0 {
        return exit::USAGE;
    }

    let file = domarinn_core::loader::resolve_suite_path(path);
    // The warning count rides the summary line so a green exit does not read as
    // "nothing to see" when there is a block of advice on stderr.
    let tail = if warnings > 0 {
        format!(" ({warnings} warning(s))")
    } else {
        String::new()
    };
    println!(
        "ok: {} — {} provider(s), {} prompt(s), {} test source(s){tail}",
        file.display(),
        suite.providers.len(),
        suite.prompts.len(),
        suite.tests.len()
    );
    exit::OK
}

fn cmd_schema(which: SchemaKind) -> u8 {
    let schema = match which {
        SchemaKind::Config => domarinn_core::config_schema(),
        SchemaKind::Result => domarinn_core::result_schema(),
    };
    match serde_json::to_string_pretty(&schema) {
        Ok(s) => {
            println!("{s}");
            exit::OK
        }
        Err(e) => {
            eprintln!("error: {e}");
            exit::INFRA
        }
    }
}

fn cmd_list(what: ListKind, path: &Path, json: bool, generators: bool) -> u8 {
    let suite = match domarinn_core::load_file(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    match what {
        ListKind::Providers => {
            if json {
                // The JSON shape stays a plain id array — it predates the
                // fallback fields and scripts index it positionally.
                let ids: Vec<&str> = suite.providers.iter().map(|p| p.id.as_str()).collect();
                print_list(&ids, json);
            } else {
                for p in &suite.providers {
                    let mut line = p.id.clone();
                    if p.fallback_only {
                        line.push_str(" (fallback_only)");
                    }
                    if !p.fallback.is_empty() {
                        line.push_str(&format!(" -> fallback: [{}]", p.fallback.join(", ")));
                    }
                    println!("{line}");
                }
            }
        }
        ListKind::Prompts => {
            let ids: Vec<&str> = suite.prompts.iter().map(|p| p.id.as_str()).collect();
            print_list(&ids, json);
        }
        ListKind::Tests => {
            let file = domarinn_core::loader::resolve_suite_path(path);
            let base_dir = domarinn_core::loader::suite_base_dir(&file);
            let expanded = match domarinn_core::expand_tests(&suite, &base_dir) {
                Ok(expanded) => expanded,
                Err(e) => {
                    eprintln!("error: {e}");
                    return exit::USAGE;
                }
            };
            let mut ids: Vec<String> = expanded.tests.iter().filter_map(|t| t.id.clone()).collect();

            // Generators only produce cases by being run, so without `--generators`
            // this listing is the inline and `file://` cases alone. On a
            // generator-driven suite that is most of the suite missing, and
            // `--filter` targets cannot be previewed at all — hence the flag, and
            // hence a note that names it rather than just counting what is absent.
            if !expanded.deferred_generators.is_empty() {
                if generators {
                    match run_generators(&expanded.deferred_generators, &base_dir) {
                        Ok(cases) => ids.extend(cases.into_iter().filter_map(|t| t.id)),
                        Err((code, message)) => {
                            eprintln!("error: {message}");
                            return code;
                        }
                    }
                } else if !json {
                    eprintln!(
                        "note: {} generator(s) produce additional tests at run time; \
                         pass --generators to run them and list their ids",
                        expanded.deferred_generators.len()
                    );
                }
            }

            let refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
            print_list(&refs, json);
        }
    }
    exit::OK
}

/// Run a suite's test generators and return the cases they produce, or the exit
/// code and message their failure earns.
///
/// The suite `defaults` merge the runner performs on generated cases is skipped
/// deliberately: it fills vars, asserts, tags, thresholds and salts, none of
/// which can change an id, and an id is the whole output here.
fn run_generators(
    specs: &[domarinn_core::config::GeneratorSpec],
    base_dir: &Path,
) -> Result<Vec<domarinn_core::config::TestCase>, (u8, String)> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| (exit::INFRA, format!("starting the async runtime: {e}")))?;
    runtime
        .block_on(domarinn_core::generate::resolve_generators(specs, base_dir))
        .map_err(|e| {
            // The same split the runner makes: a generator that produced
            // malformed tests is a config problem, one that failed to spawn or
            // timed out is infrastructure.
            let code = if matches!(e, domarinn_core::generate::GenerateError::BadTests { .. }) {
                exit::USAGE
            } else {
                exit::INFRA
            };
            (code, e.to_string())
        })
}

fn print_list(items: &[&str], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string(items).unwrap_or_else(|_| "[]".into())
        );
    } else {
        for item in items {
            println!("{item}");
        }
    }
}

fn cmd_server(port: u16, bind: IpAddr, data_dir: PathBuf) -> u8 {
    // Closed by default: anonymous access is an explicit operator choice via
    // DOMARINN_AUTH_MODE, never the out-of-the-box posture.
    let config = domarinn_server::ServerConfig {
        port,
        data_dir,
        auth_mode: domarinn_server::AuthMode::Closed,
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::INFRA;
        }
    };
    match runtime.block_on(domarinn_server::serve_on(config, bind)) {
        Ok(()) => exit::OK,
        Err(e) => fail(exit::INFRA, &e),
    }
}

fn cmd_migrate_db(data_dir: PathBuf, database_url: String) -> u8 {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::INFRA;
        }
    };
    match runtime.block_on(domarinn_server::storage::migratedb::migrate_to_postgres(
        data_dir,
        database_url,
    )) {
        Ok(report) => {
            let mut total = 0u64;
            for (table, rows) in &report.tables {
                println!("{table}: {rows} rows");
                total += rows;
            }
            println!("total: {total} rows");
            exit::OK
        }
        Err(e) => fail(exit::INFRA, &e),
    }
}

fn cmd_gen_types(dir: &Path) -> u8 {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("error: creating {}: {e}", dir.display());
        return exit::INFRA;
    }
    if let Err(e) = domarinn_core::export_types(dir) {
        eprintln!("error: exporting core types: {e}");
        return exit::INFRA;
    }
    if let Err(e) = domarinn_server::export_api_types(dir) {
        eprintln!("error: exporting server API types: {e}");
        return exit::INFRA;
    }
    println!(
        "wrote TypeScript definitions (core result/diff types + server API DTOs) to {}",
        dir.display()
    );
    exit::OK
}

/// How long the whole probe — connect, request, response — may take. Under the
/// image HEALTHCHECK's own `--timeout=5s`, so a hung server is reported by this
/// binary with a reason rather than killed by Docker without one.
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(3);

fn cmd_healthcheck(port: u16, host: Option<String>) -> u8 {
    let bind = std::env::var("DOMARINN_BIND").ok();
    let host = host.unwrap_or_else(|| healthcheck_default_host(bind.as_deref()));
    let url = healthcheck_url(&host, port);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("healthcheck failed: starting the async runtime: {e}");
            return exit::INFRA;
        }
    };
    let probe = async {
        let client = reqwest::Client::builder()
            .timeout(HEALTHCHECK_TIMEOUT)
            // Never probe through a proxy: the target is this machine.
            .no_proxy()
            .build()?;
        client.get(&url).send().await
    };
    match runtime.block_on(probe) {
        Ok(response) if response.status().is_success() => exit::OK,
        Ok(response) => {
            eprintln!("healthcheck failed: {url} returned {}", response.status());
            exit::INFRA
        }
        Err(e) => {
            // The whole source chain: reqwest's Display alone is "error
            // sending request for url (...)", which hides whether it was a
            // refusal or a timeout.
            let mut message = e.to_string();
            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                message.push_str(&format!(": {cause}"));
                source = cause.source();
            }
            eprintln!("healthcheck failed: {message}");
            exit::INFRA
        }
    }
}

/// The host `healthcheck` probes when `--host` is not given.
///
/// A server bound to a wildcard listens on loopback too, so loopback is probed
/// (of the matching family — `::` may be v6-only). A server bound to one
/// specific address does *not* answer on loopback, so that address is probed:
/// this keeps the image HEALTHCHECK green when only DOMARINN_BIND changes.
fn healthcheck_default_host(bind: Option<&str>) -> String {
    match bind.and_then(|b| b.trim().parse::<IpAddr>().ok()) {
        Some(IpAddr::V4(v4)) if v4.is_unspecified() => "127.0.0.1".to_string(),
        Some(IpAddr::V6(v6)) if v6.is_unspecified() => "::1".to_string(),
        Some(ip) => ip.to_string(),
        None => "127.0.0.1".to_string(),
    }
}

/// `http://host:port/api/v1/health`, bracketing an IPv6 literal.
fn healthcheck_url(host: &str, port: u16) -> String {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("http://[{host}]:{port}/api/v1/health")
    } else {
        format!("http://{host}:{port}/api/v1/health")
    }
}

fn cmd_completions(shell: clap_complete::Shell) -> u8 {
    let mut command = Cli::command();
    clap_complete::generate(shell, &mut command, "domarinn", &mut std::io::stdout());
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthcheck_probes_loopback_unless_bound_to_one_address() {
        assert_eq!(healthcheck_default_host(None), "127.0.0.1");
        assert_eq!(healthcheck_default_host(Some("0.0.0.0")), "127.0.0.1");
        assert_eq!(healthcheck_default_host(Some("::")), "::1");
        assert_eq!(healthcheck_default_host(Some("10.0.0.5")), "10.0.0.5");
        assert_eq!(healthcheck_default_host(Some("::1")), "::1");
        // Unparseable: the server would refuse to start on it anyway; probe
        // loopback rather than echo garbage into a URL.
        assert_eq!(healthcheck_default_host(Some("nonsense")), "127.0.0.1");
    }

    #[test]
    fn healthcheck_url_brackets_ipv6() {
        assert_eq!(
            healthcheck_url("127.0.0.1", 8321),
            "http://127.0.0.1:8321/api/v1/health"
        );
        assert_eq!(
            healthcheck_url("::1", 9000),
            "http://[::1]:9000/api/v1/health"
        );
        assert_eq!(
            healthcheck_url("[::1]", 9000),
            "http://[::1]:9000/api/v1/health"
        );
        assert_eq!(
            healthcheck_url("domarinn.internal", 80),
            "http://domarinn.internal:80/api/v1/health"
        );
    }

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
