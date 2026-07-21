//! Rendering and persisting run results.

use std::io::Write;
use std::path::Path;

use domarinn_core::result::{CaseStatus, RunResult};

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Json,
    Jsonl,
    Junit,
}

/// Persist a run under `.domarinn/runs/<run_id>/result.json` and update the
/// `latest` pointer.
pub fn persist(result: &RunResult) -> std::io::Result<()> {
    let dir = Path::new(".domarinn")
        .join("runs")
        .join(result.run_id.as_str());
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(result).map_err(std::io::Error::other)?;
    std::fs::write(dir.join("result.json"), json)?;
    std::fs::write(
        Path::new(".domarinn").join("runs").join("latest"),
        result.run_id.as_str(),
    )?;
    Ok(())
}

/// Emit a run in the requested format to `out` (or stdout).
pub fn emit(format: Format, result: &RunResult, out: Option<&Path>) -> std::io::Result<()> {
    let text = match format {
        Format::Table => render_table(result),
        Format::Json => serde_json::to_string_pretty(result).map_err(std::io::Error::other)?,
        Format::Jsonl => render_jsonl(result)?,
        Format::Junit => render_junit(result),
    };
    match out {
        Some(path) => std::fs::write(path, text.as_bytes()),
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(text.as_bytes())?;
            if !text.ends_with('\n') {
                stdout.write_all(b"\n")?;
            }
            Ok(())
        }
    }
}

fn status_glyph(status: CaseStatus) -> &'static str {
    match status {
        CaseStatus::Pass => "PASS",
        CaseStatus::Fail => "FAIL",
        CaseStatus::Error => "ERR ",
        CaseStatus::Skip => "SKIP",
    }
}

fn render_table(result: &RunResult) -> String {
    let mut out = String::new();
    let name_w = result
        .cases
        .iter()
        .map(|c| display_name(c).len())
        .max()
        .unwrap_or(4)
        .clamp(4, 60);

    out.push_str(&format!(
        "{:<6}{:<10}{:<name_w$}score\n",
        "", "provider", "test"
    ));
    for case in &result.cases {
        out.push_str(&format!(
            "{:<6}{:<10}{:<name_w$}{:.2}\n",
            status_glyph(case.status),
            truncate(&case.cell.provider_id, 9),
            truncate(&display_name(case), name_w),
            case.score,
        ));
    }
    let s = &result.summary;
    out.push_str(&format!(
        "\n{} total: {} passed, {} failed, {} errored ({} cache hits)\n",
        result.suite.as_deref().unwrap_or("run"),
        s.passed,
        s.failed,
        s.errored,
        s.cache_hits,
    ));
    out
}

fn display_name(case: &domarinn_core::result::CaseResult) -> String {
    case.name
        .clone()
        .unwrap_or_else(|| case.cell.test_id.clone())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

fn render_jsonl(result: &RunResult) -> std::io::Result<String> {
    let mut out = String::new();
    for case in &result.cases {
        out.push_str(&serde_json::to_string(case).map_err(std::io::Error::other)?);
        out.push('\n');
    }
    Ok(out)
}

/// A minimal JUnit XML report (one testcase per result cell).
fn render_junit(result: &RunResult) -> String {
    let s = &result.summary;
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"{}\">\n",
        xml_escape(result.suite.as_deref().unwrap_or("domarinn")),
        s.total,
        s.failed,
        s.errored,
    ));
    for case in &result.cases {
        let name = xml_escape(&display_name(case));
        let classname = xml_escape(&case.cell.provider_id);
        out.push_str(&format!(
            "  <testcase classname=\"{classname}\" name=\"{name}\" time=\"{:.3}\">",
            case.latency_ms as f64 / 1000.0
        ));
        match case.status {
            CaseStatus::Fail => {
                out.push_str(&format!(
                    "<failure message=\"score {:.2}\"></failure>",
                    case.score
                ));
            }
            CaseStatus::Error => {
                let msg = xml_escape(case.error.as_deref().unwrap_or("error"));
                out.push_str(&format!("<error message=\"{msg}\"></error>"));
            }
            CaseStatus::Skip => out.push_str("<skipped></skipped>"),
            CaseStatus::Pass => {}
        }
        out.push_str("</testcase>\n");
    }
    out.push_str("</testsuite>\n");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_handles_specials() {
        assert_eq!(xml_escape("a<b>&\"c"), "a&lt;b&gt;&amp;&quot;c");
    }

    #[test]
    fn truncate_shortens() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
    }
}
