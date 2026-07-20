//! measurellm — the CLI entry point.
//!
//! Exit codes: `0` all pass; `1` assertion failures / regression; `2`
//! config/usage error; `3` infrastructure error. `3` wins over `1` so CI can
//! distinguish "the model got worse" from "the harness broke".

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod cachecfg;
mod cachecmd;
mod diffcmd;
mod import;
mod loadrun;
mod output;
mod run;
mod share;

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
#[command(name = "measurellm", version, about = "A declarative LLM eval harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Increase logging verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Log format: auto, pretty, compact, json (env: MEASURELLM_LOG_FORMAT).
    /// Logs go to stderr; RUST_LOG overrides the default filter entirely.
    #[arg(long, global = true, value_enum, default_value_t = LogFormatArg::Auto)]
    log_format: LogFormatArg,

    /// Results server base URL (or set MEASURELLM_SERVER_URL).
    #[arg(long, global = true)]
    server_url: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a suite: call providers, evaluate assertions, report results.
    Run(run::RunArgs),
    /// Upload a completed run to a results server and print its URL.
    Share(share::ShareArgs),
    /// Diff two runs (regressions, fixes, output changes, significance).
    Diff(diffcmd::DiffArgs),
    /// Render a stored run in the terminal.
    View(diffcmd::ViewArgs),
    /// Manage the local response cache.
    Cache {
        #[command(subcommand)]
        cmd: cachecmd::CacheCmd,
    },
    /// Import a config from another tool into a measurellm suite.
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
        /// Path to a suite file or a directory containing measurellm.yaml.
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
    },
    /// Run the self-hostable results server + web UI.
    Server {
        #[arg(long, default_value_t = 8321)]
        port: u16,
        #[arg(long, default_value = "/data")]
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

impl From<LogFormatArg> for measurellm_logging::LogFormat {
    fn from(value: LogFormatArg) -> Self {
        match value {
            LogFormatArg::Auto => measurellm_logging::LogFormat::Auto,
            LogFormatArg::Pretty => measurellm_logging::LogFormat::Pretty,
            LogFormatArg::Compact => measurellm_logging::LogFormat::Compact,
            LogFormatArg::Json => measurellm_logging::LogFormat::Json,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let profile = match &cli.command {
        Command::Server { .. } => measurellm_logging::LogProfile::Server,
        _ => measurellm_logging::LogProfile::Cli,
    };
    measurellm_logging::init(&measurellm_logging::LogOptions {
        profile,
        verbose: cli.verbose,
        format: cli.log_format.into(),
    });

    let code = match cli.command {
        Command::Run(args) => run::execute(args, cli.server_url),
        Command::Share(args) => share::execute(args, cli.server_url),
        Command::Diff(args) => diffcmd::execute_diff(args),
        Command::View(args) => diffcmd::execute_view(args),
        Command::Cache { cmd } => cachecmd::execute(cmd),
        Command::Import { format, path } => match format {
            ImportFormat::Promptfoo => import::execute(path),
        },
        Command::GenTypes { dir } => cmd_gen_types(&dir),
        Command::Validate { path } => cmd_validate(&path),
        Command::Schema { which } => cmd_schema(which),
        Command::List { what, path, json } => cmd_list(what, &path, json),
        Command::Server { port, data_dir } => cmd_server(port, data_dir),
        Command::Healthcheck { port } => cmd_healthcheck(port),
    };
    ExitCode::from(code)
}

fn cmd_validate(path: &Path) -> u8 {
    let (suite, raw) = match measurellm_core::loader::load_file_raw(path) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let issues = measurellm_core::validate(&suite, &raw);
    if issues.is_empty() {
        let file = measurellm_core::loader::resolve_suite_path(path);
        println!(
            "ok: {} — {} provider(s), {} prompt(s), {} test source(s)",
            file.display(),
            suite.providers.len(),
            suite.prompts.len(),
            suite.tests.len()
        );
        exit::OK
    } else {
        eprintln!("{} validation issue(s):", issues.len());
        for issue in &issues {
            eprintln!("  - {issue}");
        }
        exit::USAGE
    }
}

fn cmd_schema(which: SchemaKind) -> u8 {
    let schema = match which {
        SchemaKind::Config => measurellm_core::config_schema(),
        SchemaKind::Result => measurellm_core::result_schema(),
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

fn cmd_list(what: ListKind, path: &Path, json: bool) -> u8 {
    let suite = match measurellm_core::load_file(path) {
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
            // Resolve inline + file globs; generators are listed as a count since
            // they only produce cases at run time.
            let file = measurellm_core::loader::resolve_suite_path(path);
            let base_dir = file.parent().unwrap_or_else(|| Path::new("."));
            match measurellm_core::expand_tests(&suite, base_dir) {
                Ok(expanded) => {
                    let ids: Vec<String> =
                        expanded.tests.iter().filter_map(|t| t.id.clone()).collect();
                    let refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
                    print_list(&refs, json);
                    if !expanded.deferred_generators.is_empty() && !json {
                        eprintln!(
                            "note: {} generator(s) produce additional tests at run time",
                            expanded.deferred_generators.len()
                        );
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    return exit::USAGE;
                }
            }
        }
    }
    exit::OK
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
    let config = measurellm_server::ServerConfig {
        port,
        data_dir,
        auth_mode: measurellm_server::AuthMode::Open,
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::INFRA;
        }
    };
    match runtime.block_on(measurellm_server::serve(config)) {
        Ok(()) => exit::OK,
        Err(e) => fail(exit::INFRA, &e),
    }
}

fn cmd_gen_types(dir: &Path) -> u8 {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("error: creating {}: {e}", dir.display());
        return exit::INFRA;
    }
    if let Err(e) = measurellm_core::export_types(dir) {
        eprintln!("error: exporting core types: {e}");
        return exit::INFRA;
    }
    if let Err(e) = measurellm_server::export_api_types(dir) {
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
