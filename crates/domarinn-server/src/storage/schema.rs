//! Forward-only migration SQL for both databases.

use rusqlite_migration::{Migrations, M};

/// Schema for `domarinn.db` (durable run history).
///
/// Migrations are forward-only and append-only: never edit an existing `M::up`,
/// only add new ones to the end of the vec so existing databases upgrade in
/// place. The accounts tables (migration 2) live in the runs database so they
/// share its writer-mutex and reader-pool.
pub(super) fn runs_migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            r#"
        CREATE TABLE runs (
            id                TEXT PRIMARY KEY,
            project           TEXT,
            suite             TEXT,
            created_at        INTEGER NOT NULL,
            uploaded_at       INTEGER NOT NULL,
            schema_version    INTEGER NOT NULL,
            description       TEXT,
            git_branch        TEXT,
            git_commit        TEXT,
            git_dirty         INTEGER,
            ci_provider       TEXT,
            ci_run_url        TEXT,
            case_count        INTEGER NOT NULL,
            pass_count        INTEGER NOT NULL,
            fail_count        INTEGER NOT NULL,
            error_count       INTEGER NOT NULL,
            prompt_tokens     INTEGER NOT NULL,
            completion_tokens INTEGER NOT NULL,
            cost_microusd     INTEGER,
            duration_ms       INTEGER NOT NULL,
            content_hash      TEXT NOT NULL,
            uploaded_by       TEXT
        );
        CREATE INDEX idx_runs_proj_suite_created ON runs(project, suite, created_at DESC);
        CREATE INDEX idx_runs_created ON runs(created_at DESC);
        CREATE INDEX idx_runs_branch ON runs(git_branch);

        CREATE TABLE run_tags (
            run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
            tag    TEXT NOT NULL,
            PRIMARY KEY (run_id, tag)
        );
        CREATE INDEX idx_run_tags_tag ON run_tags(tag);

        CREATE TABLE run_blobs (
            run_id   TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
            encoding TEXT NOT NULL DEFAULT 'zstd',
            body     BLOB NOT NULL
        );

        CREATE TABLE cases (
            run_id            TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
            case_key          TEXT NOT NULL,
            idx               INTEGER NOT NULL,
            name              TEXT,
            status            TEXT NOT NULL CHECK (status IN ('pass','fail','error','skip')),
            output_preview    TEXT,
            output_text       TEXT,
            output_hash       TEXT,
            asserts           TEXT,
            prompt_tokens     INTEGER,
            completion_tokens INTEGER,
            cost_microusd     INTEGER,
            latency_ms        INTEGER,
            detail            BLOB,
            PRIMARY KEY (run_id, case_key)
        );
        CREATE INDEX idx_cases_run_status ON cases(run_id, status);

        CREATE TABLE case_tags (
            run_id   TEXT NOT NULL,
            case_key TEXT NOT NULL,
            tag      TEXT NOT NULL,
            PRIMARY KEY (run_id, case_key, tag),
            FOREIGN KEY (run_id, case_key) REFERENCES cases(run_id, case_key) ON DELETE CASCADE
        );
        CREATE INDEX idx_case_tags_tag ON case_tags(run_id, tag);

        CREATE TABLE baselines (
            project TEXT NOT NULL,
            suite   TEXT NOT NULL,
            run_id  TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
            set_at  INTEGER NOT NULL,
            PRIMARY KEY (project, suite)
        );
        "#,
        ),
        M::up(
            r#"
        CREATE TABLE users (
            id            TEXT PRIMARY KEY,
            username      TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            role          TEXT NOT NULL CHECK (role IN ('admin','member')),
            disabled      INTEGER NOT NULL DEFAULT 0,
            created_at    INTEGER NOT NULL
        );

        CREATE TABLE sessions (
            token_hash   TEXT PRIMARY KEY,
            user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at   INTEGER,
            expires_at   INTEGER,
            last_used_at INTEGER
        );
        CREATE INDEX idx_sessions_user ON sessions(user_id);

        CREATE TABLE api_keys (
            id           TEXT PRIMARY KEY,
            user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name         TEXT,
            prefix       TEXT NOT NULL,
            key_hash     TEXT NOT NULL,
            scope        TEXT NOT NULL,
            created_at   INTEGER,
            last_used_at INTEGER,
            revoked      INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX idx_api_keys_prefix ON api_keys(prefix);
        CREATE INDEX idx_api_keys_user ON api_keys(user_id);
        "#,
        ),
        // Migration 3: promote matrix-cell identity, score, stop_reason, and the
        // run config digest out of the zstd blobs into queryable columns +
        // indexes. Ingest writes them going forward; `backfill` populates
        // pre-existing rows on open. `repeat_idx` (not `repeat`) sidesteps the
        // SQL keyword.
        M::up(
            r#"
        ALTER TABLE cases ADD COLUMN provider_id TEXT;
        ALTER TABLE cases ADD COLUMN prompt_id TEXT;
        ALTER TABLE cases ADD COLUMN test_id TEXT;
        ALTER TABLE cases ADD COLUMN repeat_idx INTEGER;
        ALTER TABLE cases ADD COLUMN score REAL;
        ALTER TABLE cases ADD COLUMN stop_reason TEXT;
        CREATE INDEX idx_cases_run_provider ON cases(run_id, provider_id);
        CREATE INDEX idx_cases_run_test ON cases(run_id, test_id);
        CREATE INDEX idx_cases_key ON cases(case_key);
        ALTER TABLE runs ADD COLUMN config_digest TEXT;
        CREATE INDEX idx_runs_digest ON runs(project, suite, config_digest);
        "#,
        ),
    ])
}

/// Schema for `cache.db` (disposable content-addressed cache).
pub(super) fn cache_migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(
        r#"
        CREATE TABLE cache_entries (
            key            TEXT PRIMARY KEY,
            body           BLOB NOT NULL,
            size           INTEGER NOT NULL,
            created_at     INTEGER NOT NULL,
            last_access_at INTEGER NOT NULL
        );
        CREATE INDEX idx_cache_last_access ON cache_entries(last_access_at);

        CREATE TABLE cache_counters (
            id     INTEGER PRIMARY KEY CHECK (id = 1),
            hits   INTEGER NOT NULL DEFAULT 0,
            misses INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO cache_counters (id, hits, misses) VALUES (1, 0, 0);
        "#,
    )])
}
