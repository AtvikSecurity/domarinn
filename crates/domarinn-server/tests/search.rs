//! Full-text search: the `GET /api/v1/search` endpoint, the FTS index kept in
//! step by ingest/delete, and the startup backfill that indexes rows written
//! before the FTS migration.

mod common;

use domarinn_server::runsets::RunVisibility;

use std::path::Path;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::dto::search::{SNIPPET_CLOSE, SNIPPET_OPEN};
use domarinn_server::storage::Storage;
use domarinn_server::Settings;
use rusqlite::Connection;
use tempfile::TempDir;

/// Two runs with globally-unique marker words per searchable field, so each
/// assertion can prove exactly which field matched:
/// * `s-alpha` (checkout/regression, tag `nightly`, branch `main`): one pass
///   case with `zorblatt` in the output and `gronkle` in the rendered prompt.
/// * `s-beta` (billing/smoke, tag `release`, branch `feature/fts-search`): one
///   error case with `frobnicate` in the provider error string.
fn seed_runs() -> Vec<domarinn_core::result::RunResult> {
    vec![
        make_run(
            "s-alpha",
            Some("checkout"),
            Some("regression"),
            vec!["nightly"],
            Some("main"),
            0,
            &[
                CaseSpec::new("openai", "checkout/empty-cart", CaseStatus::Pass)
                    .output(Some("the cart is empty zorblatt"))
                    .rendered_prompt("simulate an empty cart gronkle scenario"),
            ],
        ),
        make_run(
            "s-beta",
            Some("billing"),
            Some("smoke"),
            vec!["release"],
            Some("feature/fts-search"),
            10,
            &[CaseSpec::new("qwen", "billing/refund", CaseStatus::Error)
                .output(None)
                .error("upstream exploded with frobnicate 502")],
        ),
    ]
}

async fn seed(app: &axum::Router) {
    for run in seed_runs() {
        let reply = post_json(app, "/api/v1/runs", None, &run_value(&run)).await;
        assert_eq!(reply.status, StatusCode::CREATED, "seed {}", run.run_id);
    }
}

async fn delete(app: &axum::Router, uri: &str, token: Option<&str>) -> Reply {
    send(app, "DELETE", uri, token, None, Vec::new()).await
}

fn case_hits(body: &serde_json::Value) -> Vec<(String, String)> {
    body["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["run_id"].as_str().unwrap().to_string(),
                c["snippet"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn run_hit_ids(body: &serde_json::Value) -> Vec<String> {
    body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// A run's note reaches `runs.description` and its `runs_fts` slot.
///
/// Both existed before this and neither was ever written: ingest inserted NULL,
/// so the indexed column matched nothing. The note comes from `--note`, falling
/// back to the suite's `description` — which was likewise parsed and read by
/// nothing.
#[tokio::test]
async fn a_runs_note_is_stored_as_its_description_and_is_searchable() {
    let (app, dir) = test_app(Settings::default()).await;

    let mut run = make_run(
        "s-noted",
        Some("checkout"),
        Some("regression"),
        vec![],
        Some("main"),
        0,
        &[CaseSpec::new("openai", "checkout/noted", CaseStatus::Pass)],
    );
    run.origin = Some(domarinn_core::result::RunOrigin {
        note: Some("investigating quibblewort latency".to_string()),
        ..Default::default()
    });
    let reply = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(reply.status, StatusCode::CREATED);

    // The column itself, not just the index — the DTO does not expose it yet.
    let conn = Connection::open(dir.path().join("domarinn.db")).unwrap();
    let description: Option<String> = conn
        .query_row(
            "SELECT description FROM runs WHERE id = ?1",
            ["s-noted"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        description.as_deref(),
        Some("investigating quibblewort latency")
    );

    let reply = get(&app, "/api/v1/search?q=quibblewort").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(run_hit_ids(&reply.json()), vec!["s-noted"]);
}

#[tokio::test]
async fn search_finds_cases_by_output_prompt_and_error_text() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    // Output text.
    let reply = get(&app, "/api/v1/search?q=zorblatt").await;
    assert_eq!(reply.status, StatusCode::OK);
    let hits = case_hits(&reply.json());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, "s-alpha");
    assert!(
        hits[0]
            .1
            .contains(&format!("{SNIPPET_OPEN}zorblatt{SNIPPET_CLOSE}")),
        "snippet should mark the match: {}",
        hits[0].1
    );

    // Rendered prompt text (lives only inside the compressed detail blob).
    let reply = get(&app, "/api/v1/search?q=gronkle").await;
    let hits = case_hits(&reply.json());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, "s-alpha");

    // Provider error string.
    let reply = get(&app, "/api/v1/search?q=frobnicate").await;
    let hits = case_hits(&reply.json());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, "s-beta");
}

#[tokio::test]
async fn search_finds_runs_by_branch_tag_and_project() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    // Branch: `feature/fts-search` tokenizes to feature/fts/search.
    let reply = get(&app, "/api/v1/search?q=fts").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(run_hit_ids(&reply.json()), vec!["s-beta"]);

    // Run tag.
    let reply = get(&app, "/api/v1/search?q=nightly").await;
    assert_eq!(run_hit_ids(&reply.json()), vec!["s-alpha"]);

    // Project — also matches the beta case via its name (`qwen::billing/refund`).
    let reply = get(&app, "/api/v1/search?q=billing").await;
    let body = reply.json();
    assert_eq!(run_hit_ids(&body), vec!["s-beta"]);
    assert_eq!(case_hits(&body).len(), 1);

    // Prefix matching: a partial token still hits.
    let reply = get(&app, "/api/v1/search?q=zorb").await;
    assert_eq!(case_hits(&reply.json()).len(), 1);
}

#[tokio::test]
async fn search_run_hits_carry_display_fields_and_case_hits_carry_status() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let body = get(&app, "/api/v1/search?q=fts").await.json();
    let run = &body["runs"][0];
    assert_eq!(run["project"], "billing");
    assert_eq!(run["suite"], "smoke");
    assert!(run["created_at"]
        .as_str()
        .unwrap()
        .starts_with("2026-01-01T"));

    let body = get(&app, "/api/v1/search?q=frobnicate").await.json();
    let case = &body["cases"][0];
    assert_eq!(case["status"], "error");
    assert_eq!(case["project"], "billing");
    assert_eq!(case["suite"], "smoke");
    assert_eq!(case["name"], "qwen::billing/refund");
    assert!(case["case_key"].as_str().is_some());
}

#[tokio::test]
async fn search_survives_fts_operators_and_junk_input() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    for q in [
        "AND",
        "%22%28%28",        // `"((`
        "a%20OR%20NOT%20(", // `a OR NOT (`
        "--",
        "%2A", // `*`
    ] {
        let reply = get(&app, &format!("/api/v1/search?q={q}")).await;
        assert_eq!(reply.status, StatusCode::OK, "q={q} must not 500");
    }

    // Empty / whitespace-only queries return empty groups, not errors.
    let reply = get(&app, "/api/v1/search?q=%20%20").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["runs"].as_array().unwrap().len(), 0);
    assert_eq!(body["cases"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn deleting_a_run_removes_its_search_rows() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    assert_eq!(
        case_hits(&get(&app, "/api/v1/search?q=frobnicate").await.json()).len(),
        1
    );

    let reply = delete(&app, "/api/v1/runs/s-beta", None).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);

    let body = get(&app, "/api/v1/search?q=frobnicate").await.json();
    assert_eq!(case_hits(&body).len(), 0);
    let body = get(&app, "/api/v1/search?q=fts").await.json();
    assert_eq!(run_hit_ids(&body).len(), 0);
}

/// Simulates a database created before the FTS migration: ingest normally,
/// wipe the FTS tables out-of-band, and reopen. The startup backfill must
/// reindex everything — including prompt/error text that only exists inside
/// the compressed `detail` blobs.
#[tokio::test]
async fn reopen_backfills_search_rows_for_unindexed_runs() {
    let dir = TempDir::new().unwrap();
    {
        let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
        for run in seed_runs() {
            storage.ingest_run(run, None).await.unwrap();
        }
    }
    wipe_fts(dir.path());
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();

    let res = storage
        .search("gronkle".to_string(), 20, RunVisibility::Full)
        .await
        .unwrap();
    assert_eq!(res.cases.len(), 1, "prompt text reindexed from the blob");
    let res = storage
        .search("frobnicate".to_string(), 20, RunVisibility::Full)
        .await
        .unwrap();
    assert_eq!(res.cases.len(), 1, "error text reindexed from the blob");
    let res = storage
        .search("nightly".to_string(), 20, RunVisibility::Full)
        .await
        .unwrap();
    assert_eq!(res.runs.len(), 1, "run metadata reindexed");
}

fn wipe_fts(dir: &Path) {
    let conn = Connection::open(dir.join("domarinn.db")).expect("open raw runs db");
    conn.execute("DELETE FROM runs_fts", []).unwrap();
    conn.execute("DELETE FROM cases_fts", []).unwrap();
}
