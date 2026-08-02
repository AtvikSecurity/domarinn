//! Compact text renderings of tool results.
//!
//! Every tool result carries both `structuredContent` (the full JSON, for
//! programmatic clients) and a `content[0].text` block. The convention is to
//! serialize the JSON into that text block, which doubles the payload — so
//! list-shaped tools render an aligned table instead. A table is roughly 3-5x
//! cheaper than the equivalent JSON and reads better to a model. Object-shaped
//! results keep the JSON, where client support for `structuredContent` alone
//! is not universal enough to drop it.

use serde_json::Value;

use super::budget::{fence, UNTRUSTED_WARNING};

/// A column: header, and how to pull its cell out of a row.
struct Column<'a> {
    header: &'a str,
    key: &'a str,
}

/// Render rows as a space-aligned table. Empty input renders as a single
/// explicit line — a model reading "(no rows)" behaves far better than one
/// reading an empty string.
fn table(rows: &Value, columns: &[Column<'_>]) -> String {
    let Some(rows) = rows.as_array() else {
        return "(no rows)".to_string();
    };
    if rows.is_empty() {
        return "(no rows)".to_string();
    }

    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| columns.iter().map(|c| scalar(&row[c.key])).collect())
        .collect();

    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            cells
                .iter()
                .map(|row| row[i].chars().count())
                .chain(std::iter::once(c.header.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let mut out = String::new();
    push_row(
        &mut out,
        &columns
            .iter()
            .map(|c| c.header.to_string())
            .collect::<Vec<_>>(),
        &widths,
    );
    for row in &cells {
        push_row(&mut out, row, &widths);
    }
    out.trim_end().to_string()
}

fn push_row(out: &mut String, cells: &[String], widths: &[usize]) {
    let last = cells.len().saturating_sub(1);
    for (i, cell) in cells.iter().enumerate() {
        if i == last {
            out.push_str(cell);
        } else {
            let pad = widths[i].saturating_sub(cell.chars().count());
            out.push_str(cell);
            out.push_str(&" ".repeat(pad + 2));
        }
    }
    out.push('\n');
}

/// A JSON scalar as a compact cell. `null` renders as `-` so columns stay
/// aligned and absence is visible.
fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a.len().to_string(),
        Value::Object(_) => "{…}".to_string(),
    }
}

/// Pretty JSON, for object-shaped results where a table would lose structure.
pub fn json_block(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
}

pub fn runs_table(runs: &Value) -> String {
    let body = table(
        runs,
        &[
            Column {
                header: "id",
                key: "id",
            },
            Column {
                header: "project",
                key: "project",
            },
            Column {
                header: "suite",
                key: "suite",
            },
            Column {
                header: "pass",
                key: "pass_count",
            },
            Column {
                header: "fail",
                key: "fail_count",
            },
            Column {
                header: "err",
                key: "error_count",
            },
            Column {
                header: "rate",
                key: "pass_rate",
            },
            Column {
                header: "branch",
                key: "git_branch",
            },
            Column {
                header: "created_at",
                key: "created_at",
            },
        ],
    );
    format!("runs (newest first)\n{body}")
}

pub fn projects_table(projects: &Value) -> String {
    let body = table(
        projects,
        &[
            Column {
                header: "project",
                key: "project",
            },
            Column {
                header: "runs",
                key: "run_count",
            },
            Column {
                header: "suites",
                key: "suite_count",
            },
            Column {
                header: "last_run_at",
                key: "last_run_at",
            },
        ],
    );
    format!("projects\n{body}")
}

pub fn suites_table(project: &str, suites: &Value) -> String {
    let body = table(
        suites,
        &[
            Column {
                header: "suite",
                key: "suite",
            },
            Column {
                header: "runs",
                key: "run_count",
            },
            Column {
                header: "last_run_at",
                key: "last_run_at",
            },
        ],
    );
    format!("suites in project '{project}'\n{body}")
}

pub fn run_summary(structured: &Value) -> String {
    json_block(structured)
}

/// Case lists carry `output_preview`, which is model-generated and therefore
/// untrusted — so the whole table is fenced and warned about.
pub fn cases_table(cases: &Value, run_id: &str) -> String {
    let body = table(
        cases,
        &[
            Column {
                header: "case_key",
                key: "case_key",
            },
            Column {
                header: "status",
                key: "status",
            },
            Column {
                header: "score",
                key: "score",
            },
            Column {
                header: "provider",
                key: "provider_id",
            },
            Column {
                header: "prompt",
                key: "prompt_id",
            },
            Column {
                header: "test",
                key: "test_id",
            },
            Column {
                header: "stop",
                key: "stop_reason",
            },
            // Why the preview is blank, when it is. Absent on every case that
            // produced output, which is most of them, so it reads as `-`.
            Column {
                header: "empty",
                key: "empty_reason",
            },
            Column {
                header: "preview",
                key: "output_preview",
            },
        ],
    );
    format!(
        "cases in run {run_id}\n{}\n\n{UNTRUSTED_WARNING}",
        fence(&body, "stored_model_output", run_id)
    )
}

pub fn case_detail(detail: &Value, case_key: &str) -> String {
    format!(
        "{}\n\n{UNTRUSTED_WARNING}",
        fence(&json_block(detail), "stored_model_output", case_key)
    )
}

pub fn history_table(points: &Value, case_key: &str) -> String {
    let body = table(
        points,
        &[
            Column {
                header: "run_id",
                key: "run_id",
            },
            Column {
                header: "status",
                key: "status",
            },
            Column {
                header: "score",
                key: "score",
            },
            Column {
                header: "created_at",
                key: "created_at",
            },
        ],
    );
    format!("history for case {case_key} (newest first)\n{body}")
}

pub fn compare_table(base: &str, head: &str, rows: &Value, summary: &Value) -> String {
    let body = table(
        rows,
        &[
            Column {
                header: "case_key",
                key: "case_key",
            },
            Column {
                header: "delta",
                key: "delta",
            },
            Column {
                header: "base",
                key: "base_status",
            },
            Column {
                header: "head",
                key: "head_status",
            },
            Column {
                header: "score_delta",
                key: "score_delta",
            },
            Column {
                header: "change",
                key: "change",
            },
        ],
    );
    format!(
        "compare {base} -> {head}\nsummary: {}\n\n{body}",
        json_block(summary)
    )
}

/// Search hits quote stored output, so the same fencing applies.
pub fn search_text(structured: &Value, query: &str) -> String {
    format!(
        "search results for {query:?}\n{}\n\n{UNTRUSTED_WARNING}",
        fence(&json_block(structured), "stored_model_output", query)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_an_aligned_table() {
        let rows = json!([
            { "id": "r1", "pass_count": 3 },
            { "id": "run-long", "pass_count": 10 },
        ]);
        let out = table(
            &rows,
            &[
                Column {
                    header: "id",
                    key: "id",
                },
                Column {
                    header: "pass",
                    key: "pass_count",
                },
            ],
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("id"));
        // Every row's second column starts at the same offset.
        let offset = lines[1].find('3').unwrap();
        assert_eq!(lines[2].find("10").unwrap(), offset);
    }

    #[test]
    fn empty_input_is_explicit() {
        assert_eq!(table(&json!([]), &[]), "(no rows)");
        assert_eq!(table(&Value::Null, &[]), "(no rows)");
    }

    #[test]
    fn null_cells_render_as_a_dash() {
        let out = table(
            &json!([{ "suite": null }]),
            &[Column {
                header: "suite",
                key: "suite",
            }],
        );
        assert!(out.ends_with('-'));
    }

    #[test]
    fn missing_keys_do_not_panic() {
        let out = table(
            &json!([{ "a": 1 }]),
            &[Column {
                header: "b",
                key: "b",
            }],
        );
        assert!(out.contains('-'));
    }

    #[test]
    fn case_tables_are_fenced_and_warned() {
        let out = cases_table(&json!([{ "case_key": "c1", "output_preview": "hi" }]), "r1");
        assert!(out.contains("<untrusted source=\"stored_model_output\""));
        assert!(out.contains("</untrusted>"));
        assert!(out.contains(UNTRUSTED_WARNING));
    }
}
