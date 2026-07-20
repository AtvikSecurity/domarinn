//! measurellm — the CLI entry point.
//!
//! Exit codes: `0` all pass; `1` assertion failures / regression; `2`
//! config/usage error; `3` infrastructure error. `3` wins over `1` so CI can
//! distinguish "the model got worse" from "the harness broke".

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Exit codes with meaning to CI.
#[allow(dead_code)] // ASSERT_FAIL is used once `run` lands in Phase 2.
mod exit {
    pub const OK: u8 = 0;
    pub const ASSERT_FAIL: u8 = 1;
    pub const USAGE: u8 = 2;
    pub const INFRA: u8 = 3;
}

#[derive(Parser)]
#[command(name = "measurellm", version, about = "A declarative LLM eval harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Increase logging verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Command {
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let code = match cli.command {
        Command::Validate { path } => cmd_validate(&path),
        Command::Schema { which } => cmd_schema(which),
        Command::List { what, path, json } => cmd_list(what, &path, json),
        Command::Server { port, data_dir } => cmd_server(port, data_dir),
        Command::Healthcheck { port } => cmd_healthcheck(port),
    };
    ExitCode::from(code)
}

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| format!("measurellm={level}"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn cmd_validate(path: &Path) -> u8 {
    let suite = match measurellm_core::load_file(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let issues = measurellm_core::validate(&suite);
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
            // Full test resolution (globs, generators) lands in Phase 1; for now
            // list inline test ids and note the unresolved sources.
            let mut ids: Vec<String> = Vec::new();
            let mut unresolved = 0;
            for (i, source) in suite.tests.iter().enumerate() {
                match source {
                    measurellm_core::config::TestSource::Inline(tc) => {
                        ids.push(tc.id.clone().unwrap_or_else(|| format!("inline/{i}")));
                    }
                    _ => unresolved += 1,
                }
            }
            let refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
            print_list(&refs, json);
            if unresolved > 0 && !json {
                eprintln!("note: {unresolved} test source(s) require resolution (Phase 1)");
            }
        }
    }
    exit::OK
}

fn print_list(items: &[&str], json: bool) {
    if json {
        println!("{}", serde_json::to_string(items).unwrap_or_else(|_| "[]".into()));
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
        Err(e) => {
            eprintln!("server error: {e}");
            exit::INFRA
        }
    }
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
