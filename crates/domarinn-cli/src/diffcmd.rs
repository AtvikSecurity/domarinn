//! The `diff` and `view` commands, and the `--against` comparison rendering.

use std::path::PathBuf;

use clap::Args;
use domarinn_core::diff::{diff_runs, RunDiff};
use domarinn_core::result::{CaseResult, RunResult};

use crate::casedetail;
use crate::diffrender::{render_markdown, render_table, DiffScope};
use crate::exit;
use crate::loadrun::load_run;
use crate::output::{self, Format};
use crate::style::Palette;

#[derive(Args)]
pub struct DiffArgs {
    /// Baseline run (id, path, or `latest`).
    pub base: String,
    /// Head run (id, path, or `latest`).
    pub head: String,
    /// Output format: table, json, md.
    #[arg(long, value_enum, default_value = "table")]
    pub format: DiffFormat,
    /// Which cases get an inline output diff.
    #[arg(long, value_enum, default_value_t = DiffScope::Regressions)]
    pub diffs: DiffScope,
    /// Do not truncate inline output diffs.
    #[arg(long)]
    pub full: bool,
    /// Diff the full config snapshot (default: digest note + prompts section only).
    #[arg(long)]
    pub config_diff: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq)]
pub enum DiffFormat {
    Table,
    Json,
    Md,
}

pub fn execute_diff(args: DiffArgs, palette: Palette) -> u8 {
    let base = match load_run(&args.base) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let head = match load_run(&args.head) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let diff = diff_runs(&base, &head);
    // The JSON format is the machine wire type and stays byte-identical to a raw
    // `to_string_pretty(&diff)`; markdown carries no color. Only the table is
    // palette-aware, so route it through the colored writer (Windows-safe).
    let exit_code = if diff.has_regression() {
        exit::ASSERT_FAIL
    } else {
        exit::OK
    };
    match args.format {
        DiffFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&diff).unwrap_or_default()
        ),
        DiffFormat::Md => println!("{}", render_markdown(&base, &head, &diff)),
        DiffFormat::Table => {
            let text = render_table(
                &base,
                &head,
                &diff,
                args.diffs,
                args.full,
                args.config_diff,
                &palette,
            );
            if let Err(e) = output::write_colored_stdout(&text, palette) {
                eprintln!("error: {e}");
                return exit::INFRA;
            }
        }
    }
    exit_code
}

#[derive(Args)]
pub struct ViewArgs {
    /// Run to view (id, path, or `latest`).
    #[arg(default_value = "latest")]
    pub run: String,
    /// Output format: table, json, jsonl, junit, md.
    #[arg(long, value_enum)]
    pub format: Option<Format>,
    /// Show only failed/errored cases. The table footer still summarizes the
    /// whole run; json/jsonl/junit emit only the filtered cases.
    #[arg(long)]
    pub failed: bool,
    /// Show full detail for matching case(s): case_key, case_key prefix
    /// (≥4 chars), test id, or name substring (repeatable).
    #[arg(long = "case")]
    pub cases: Vec<String>,
    /// With --case: include raw provider metadata (v2 runs).
    #[arg(long)]
    pub raw: bool,
}

pub fn execute_view(args: ViewArgs, palette: Palette) -> u8 {
    let run = match load_run(&args.run) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    // `--case` switches from the run-wide table to per-case detail; without it the
    // existing whole-run rendering is untouched. (`view` has no `--out`; the
    // detail view writes only to stdout.)
    if args.cases.is_empty() {
        let format = args.format.unwrap_or(Format::Table);
        if let Err(e) = output::emit(format, &run, None, palette, args.failed) {
            eprintln!("error: {e}");
            return exit::INFRA;
        }
        return exit::OK;
    }
    execute_view_cases(&run, &args, palette)
}

/// The `view --case` path: resolve the selectors, apply `--failed`, and render
/// the matched cases in the requested format.
///
/// `junit`/`md` are rejected (exit 2): a per-case detail dump has no meaningful
/// JUnit testsuite or run-summary-markdown form. `table` prints the human detail
/// blocks; `json` always emits an array (even for one case, so `jq` is
/// predictable) and `jsonl` one case per line.
fn execute_view_cases(run: &RunResult, args: &ViewArgs, palette: Palette) -> u8 {
    let format = args.format.unwrap_or(Format::Table);
    let unsupported = match format {
        Format::Junit => Some("junit"),
        Format::Md => Some("md"),
        _ => None,
    };
    if let Some(name) = unsupported {
        eprintln!(
            "error: view --case does not support --format {name} \
             (per-case detail has no {name} form); use table, json, or jsonl"
        );
        return exit::USAGE;
    }

    let matched = casedetail::select_union(run, &args.cases);
    if matched.is_empty() {
        eprintln!(
            "error: no case matches selector(s): {}",
            args.cases.join(", ")
        );
        let suggestions = casedetail::suggestions(run, &args.cases);
        if !suggestions.is_empty() {
            eprintln!("closest cases:");
            for s in suggestions {
                eprintln!("  {s}");
            }
        }
        return exit::USAGE;
    }

    // `--failed` intersects the selection across every format.
    let cases: Vec<&CaseResult> = if args.failed {
        matched
            .into_iter()
            .filter(|c| casedetail::is_failed(c))
            .collect()
    } else {
        matched
    };

    match format {
        Format::Json => match serde_json::to_string_pretty(&cases) {
            Ok(s) => {
                println!("{s}");
                exit::OK
            }
            Err(e) => {
                eprintln!("error: {e}");
                exit::INFRA
            }
        },
        Format::Jsonl => {
            let mut out = String::new();
            for case in &cases {
                match serde_json::to_string(case) {
                    Ok(s) => {
                        out.push_str(&s);
                        out.push('\n');
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        return exit::INFRA;
                    }
                }
            }
            print!("{out}");
            exit::OK
        }
        // Table (and the default): the human detail view.
        _ => {
            if cases.is_empty() {
                // The selectors matched, but `--failed` filtered everything out.
                println!("no matching failed/errored cases");
                return exit::OK;
            }
            let mut out = String::new();
            for (i, case) in cases.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&casedetail::render_case_detail(case, &palette, args.raw));
            }
            if let Err(e) = output::write_colored_stdout(&out, palette) {
                eprintln!("error: {e}");
                return exit::INFRA;
            }
            exit::OK
        }
    }
}

/// Write a markdown run summary (pass/fail counts + any baseline comparison).
///
/// A thin wrapper over [`crate::outputmd::render_run_md`] — the reusable core is
/// shared with `run --format md` / `view --format md` — plus the optional
/// baseline comparison (base run + diff) that only the `--summary-md` path
/// carries.
pub fn write_summary_md(
    path: &PathBuf,
    head: &RunResult,
    comparison: Option<(&RunResult, &RunDiff)>,
) -> std::io::Result<()> {
    std::fs::write(path, crate::cisummary::render(head, comparison))
}
