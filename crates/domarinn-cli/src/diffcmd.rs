//! The `diff` and `view` commands, and the `--against` comparison rendering.

use std::path::PathBuf;

use clap::Args;
use domarinn_core::diff::{diff_runs, Delta, RunDiff};
use domarinn_core::result::RunResult;

use crate::exit;
use crate::loadrun::load_run;
use crate::output::{self, Format};

#[derive(Args)]
pub struct DiffArgs {
    /// Baseline run (id, path, or `latest`).
    pub base: String,
    /// Head run (id, path, or `latest`).
    pub head: String,
    /// Output format: table, json, md.
    #[arg(long, value_enum, default_value = "table")]
    pub format: DiffFormat,
}

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq)]
pub enum DiffFormat {
    Table,
    Json,
    Md,
}

pub fn execute_diff(args: DiffArgs) -> u8 {
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
    let text = match args.format {
        DiffFormat::Json => serde_json::to_string_pretty(&diff).unwrap_or_default(),
        DiffFormat::Md => render_markdown(&diff),
        DiffFormat::Table => render_table(&diff),
    };
    println!("{text}");
    if diff.has_regression() {
        exit::ASSERT_FAIL
    } else {
        exit::OK
    }
}

#[derive(Args)]
pub struct ViewArgs {
    /// Run to view (id, path, or `latest`).
    #[arg(default_value = "latest")]
    pub run: String,
    /// Output format: table, json, jsonl, junit.
    #[arg(long, value_enum)]
    pub format: Option<Format>,
}

pub fn execute_view(args: ViewArgs) -> u8 {
    let run = match load_run(&args.run) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let format = args.format.unwrap_or(Format::Table);
    if let Err(e) = output::emit(format, &run, None) {
        eprintln!("error: {e}");
        return exit::INFRA;
    }
    exit::OK
}

fn render_table(diff: &RunDiff) -> String {
    let s = &diff.summary;
    let mut out = String::new();
    out.push_str("comparison vs baseline:\n");
    for case in &diff.cases {
        let marker = match case.delta {
            Delta::NewlyFailing => "REGRESS",
            Delta::NewlyPassing => "FIXED  ",
            Delta::Added => "NEW    ",
            Delta::Removed => "REMOVED",
            _ => continue,
        };
        out.push_str(&format!(
            "  {marker}  {}\n",
            case.name
                .clone()
                .unwrap_or_else(|| case.case_key.to_string())
        ));
    }
    out.push_str(&format!(
        "\n{} newly failing, {} newly passing, {} still failing, {} output-changed, {} added, {} removed\n",
        s.newly_failing, s.newly_passing, s.still_failing, s.output_changed, s.added, s.removed
    ));
    out.push_str(&format!(
        "McNemar: {} regressions vs {} fixes, statistic {:.2}{}\n",
        diff.mcnemar.regressions,
        diff.mcnemar.fixes,
        diff.mcnemar.statistic,
        if diff.mcnemar.significant {
            " (significant at 95%)"
        } else {
            ""
        }
    ));
    out
}

/// A markdown comparison suitable for a PR comment.
pub fn render_markdown(diff: &RunDiff) -> String {
    let s = &diff.summary;
    let mut out = String::new();
    let verdict = if diff.has_regression() {
        "❌ Regressions detected"
    } else {
        "✅ No regressions"
    };
    out.push_str(&format!("### domarinn comparison — {verdict}\n\n"));
    out.push_str("| metric | count |\n|---|---|\n");
    out.push_str(&format!("| Newly failing | {} |\n", s.newly_failing));
    out.push_str(&format!("| Newly passing | {} |\n", s.newly_passing));
    out.push_str(&format!("| Still failing | {} |\n", s.still_failing));
    out.push_str(&format!("| Output changed | {} |\n", s.output_changed));
    out.push_str(&format!("| Added | {} |\n", s.added));
    out.push_str(&format!("| Removed | {} |\n", s.removed));
    if diff.mcnemar.significant {
        out.push_str(&format!(
            "\n> McNemar test: change is statistically significant (statistic {:.2}).\n",
            diff.mcnemar.statistic
        ));
    }
    let regressions: Vec<&str> = diff
        .cases
        .iter()
        .filter(|c| c.delta == Delta::NewlyFailing)
        .filter_map(|c| c.name.as_deref())
        .collect();
    if !regressions.is_empty() {
        out.push_str("\n**Newly failing:**\n");
        for name in regressions {
            out.push_str(&format!("- {name}\n"));
        }
    }
    out
}

/// Write a markdown run summary (pass/fail counts + any baseline comparison).
pub fn write_summary_md(
    path: &PathBuf,
    run: &RunResult,
    comparison: Option<&RunDiff>,
) -> std::io::Result<()> {
    let s = &run.summary;
    let mut out = String::new();
    out.push_str(&format!(
        "### domarinn run — {} passed, {} failed, {} errored\n\n",
        s.passed, s.failed, s.errored
    ));
    let rate = domarinn_core::stats::wilson(s.passed, s.total, domarinn_core::stats::Z_95);
    out.push_str(&format!(
        "Pass rate: **{:.1}%** (95% CI {:.1}%–{:.1}%, n={})\n",
        rate.rate * 100.0,
        rate.lower * 100.0,
        rate.upper * 100.0,
        rate.total
    ));
    if let Some(diff) = comparison {
        out.push('\n');
        out.push_str(&render_markdown(diff));
    }
    std::fs::write(path, out)
}
