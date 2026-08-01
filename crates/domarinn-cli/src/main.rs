//! domarinn — the CLI entry point.
//!
//! Exit codes: `0` all pass; `1` assertion failures / regression; `2`
//! config/usage error; `3` infrastructure error. `3` wins over `1` so CI can
//! distinguish "the model got worse" from "the harness broke".

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod baseline;
mod cachecfg;
mod cachecmd;
mod cachels;
mod casedetail;
mod cisummary;
mod diffcmd;
mod diffrender;
mod import;
mod loadrun;
mod output;
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
        #[arg(value_enum)]
        format: ImportFormat,
        path: PathBuf,
    },
    /// Generate TypeScript type definitions for the result/diff DTOs.
    GenTypes {
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
    Server {
        #[arg(long, default_value_t = 8321)]
        port: u16,
        /// State directory holding the SQLite databases (env: DOMARINN_DATA_DIR).
        #[arg(long, env = "DOMARINN_DATA_DIR", default_value = "/data")]
        data_dir: PathBuf,
    },
    /// Probe this binary's own /api/v1/health (used by container HEALTHCHECK).
    Healthcheck {
        #[arg(long, default_value_t = 8321)]
        port: u16,
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
        Command::Server { port, data_dir } => cmd_server(port, data_dir),
        Command::Healthcheck { port } => cmd_healthcheck(port),
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
            eprintln!("{} {e}", palette.error("error:"));
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
                domarinn_core::Severity::Error => palette.error("error:"),
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
            let ids: Vec<&str> = suite.providers.iter().map(|p| p.id.as_str()).collect();
            print_list(&ids, json);
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

fn cmd_server(port: u16, data_dir: PathBuf) -> u8 {
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
    match runtime.block_on(domarinn_server::serve(config)) {
        Ok(()) => exit::OK,
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

fn cmd_healthcheck(port: u16) -> u8 {
    // Minimal dependency-free probe: open a TCP connection to the port. A full
    // HTTP probe lands with the client work in Phase 3.
    let addr = format!("127.0.0.1:{port}");
    match std::net::TcpStream::connect(&addr) {
        Ok(_) => exit::OK,
        Err(e) => {
            eprintln!("healthcheck failed: {e}");
            exit::INFRA
        }
    }
}
