//! Consolidated Postgres schema, version 1.
//!
//! The Postgres backend is new, so it starts from one consolidated snapshot of
//! `schema.rs`'s **final** state — every SQLite migration up to and including
//! 19 on the runs side and 3 on the cache side, folded into a single v1 per
//! table set. There are no historical Postgres databases to upgrade, so
//! replaying the SQLite ledger (with its two table rebuilds) would create
//! intermediate states nothing ever observes.
//!
//! # The mirroring rule
//!
//! From here on the two ledgers advance together: **every future migration
//! appended to `schema.rs` MUST be mirrored here as a new version** in
//! [`RUNS_MIGRATIONS`] or [`CACHE_MIGRATIONS`]. Version numbers in this file
//! are its own ledger and do not correspond to `schema.rs` migration numbers —
//! runs v1 here equals SQLite runs migrations 1–19.
//!
//! # Type translation
//!
//! * `INTEGER` → `BIGINT` everywhere, including 0/1 flag columns
//!   (`disabled`, `revoked`, `cached`, `index_ok`, `git_dirty`). **No BOOLEAN
//!   columns**: shared queries compare `= 1` and must stay portable.
//! * `REAL` → `DOUBLE PRECISION`; `BLOB` → `BYTEA`; `TEXT` → `TEXT`.
//!   JSON-carrying TEXT (`cases.asserts`) stays TEXT, never jsonb.
//! * PRIMARY KEY / FOREIGN KEY / ON DELETE CASCADE / CHECK: copied 1:1.
//! * Partial-index predicates are ported verbatim (Postgres, like SQLite, only
//!   uses a partial index when the query's WHERE implies the predicate — the
//!   no-COALESCE warnings from SQLite migrations 11/13 still bind).
//! * The `COALESCE(suite,'')` expression unique indexes are ported verbatim so
//!   the shared `ON CONFLICT(project, COALESCE(suite,'') …)` targets resolve.
//!
//! # FTS translation
//!
//! SQLite's FTS5 virtual tables become plain tables with a `tsv tsvector`
//! generated column and a GIN index. Their visible column names/positions
//! accept the exact shared INSERTs in `storage::search` and
//! `storage::cacheindex`; only the MATCH/snippet/rank query fragments branch by
//! dialect. `cache_entries_fts` rows are keyed by `cache_entries.id` (the
//! Postgres spelling of SQLite's rowid alignment), and its FK cascade replaces
//! SQLite's `cache_entries_delete_fts` trigger.

/// Forward-only migrations for the runs table set. Version numbers are this
/// file's own ledger (they do not correspond to schema.rs migration numbers).
pub(super) const RUNS_MIGRATIONS: &[(i64, &str)] = &[(1, RUNS_V1)];

/// Forward-only migrations for the cache table set (its own ledger, matching
/// the two-database split on SQLite; on Postgres both sets share one database
/// — the names are disjoint).
pub(super) const CACHE_MIGRATIONS: &[(i64, &str)] = &[(1, CACHE_V1)];

/// Final state of SQLite runs migrations 1–19, as one snapshot.
const RUNS_V1: &str = r#"
-- runs: columns are SQLite migration 1 plus every later ALTER (3, 6, 8, 9,
-- 12, 15, 18, 19). Nullable promoted columns carry the sentinel contracts
-- documented in schema.rs: NULL = unknown/not-backfilled, -1 = undecodable
-- blob (cache_hits/cache_misses/empty_count/fallback_count), and readers
-- must never flatten either into a real answer.
CREATE TABLE runs (
    id                     TEXT PRIMARY KEY,
    project                TEXT,
    suite                  TEXT,
    created_at             BIGINT NOT NULL,
    uploaded_at            BIGINT NOT NULL,
    schema_version         BIGINT NOT NULL,
    description            TEXT,
    git_branch             TEXT,
    git_commit             TEXT,
    git_dirty              BIGINT,
    ci_provider            TEXT,
    ci_run_url             TEXT,
    case_count             BIGINT NOT NULL,
    pass_count             BIGINT NOT NULL,
    fail_count             BIGINT NOT NULL,
    error_count            BIGINT NOT NULL,
    prompt_tokens          BIGINT NOT NULL,
    completion_tokens      BIGINT NOT NULL,
    cost_microusd          BIGINT,
    duration_ms            BIGINT NOT NULL,
    content_hash           TEXT NOT NULL,
    uploaded_by            TEXT,
    config_digest          TEXT,
    cache_hits             BIGINT,
    cache_misses           BIGINT,
    actor                  TEXT,
    host                   TEXT,
    domarinn_version       TEXT,
    prompts_digest         TEXT,
    providers_digest       TEXT,
    tests_digest           TEXT,
    asserts_digest         TEXT,
    grader_digest          TEXT,
    cache_read_tokens      BIGINT,
    cache_write_tokens     BIGINT,
    cache_savings_microusd BIGINT,
    grader_cost_microusd   BIGINT,
    empty_count            BIGINT,
    fallback_count         BIGINT,
    xfail_count            BIGINT,
    xpass_count            BIGINT
);
CREATE INDEX idx_runs_proj_suite_created ON runs(project, suite, created_at DESC);
CREATE INDEX idx_runs_created ON runs(created_at DESC);
CREATE INDEX idx_runs_branch ON runs(git_branch);
CREATE INDEX idx_runs_digest ON runs(project, suite, config_digest);
CREATE INDEX idx_runs_actor ON runs(actor, created_at DESC);
CREATE INDEX idx_runs_ci ON runs(ci_provider, created_at DESC);
CREATE INDEX idx_runs_uploaded_by ON runs(uploaded_by, created_at DESC);
-- Not a prefix of idx_runs_proj_suite_created: that one still serves
-- branch-agnostic ordering without a sort, so both earn their keep.
CREATE INDEX idx_runs_proj_suite_branch_created
    ON runs(project, suite, git_branch, created_at DESC);
-- Partial, matching the cached=exclude predicate exactly (FULLY_CACHED in
-- runs.rs). The planner only uses it when the query's WHERE implies this
-- predicate, so no COALESCE may appear on either side — same rule as SQLite
-- migrations 11/16. This is migration 16's verdict-free form; migration 11's
-- idx_runs_cached_passing was dropped there and is deliberately absent here.
CREATE INDEX idx_runs_fully_cached ON runs(created_at DESC)
    WHERE cache_misses = 0 AND cache_hits > 0;

CREATE TABLE run_tags (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    tag    TEXT NOT NULL,
    PRIMARY KEY (run_id, tag)
);
CREATE INDEX idx_run_tags_tag ON run_tags(tag);

CREATE TABLE run_blobs (
    run_id   TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    encoding TEXT NOT NULL DEFAULT 'zstd',
    body     BYTEA NOT NULL
);

-- cases: the SQLite migration-19 rebuild's column set (migration 1 plus the
-- ALTERs from 3, 6, 7, 10, 13, 15, 17), with the six-status CHECK. The ''
-- sentinels on error/empty_reason/answered_by_provider_id mean "decoded,
-- nothing there"; NULL means "not yet backfilled" (see storage::backfill).
CREATE TABLE cases (
    run_id                  TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    case_key                TEXT NOT NULL,
    idx                     BIGINT NOT NULL,
    name                    TEXT,
    status                  TEXT NOT NULL CHECK
        (status IN ('pass','fail','error','skip','xfail','xpass')),
    output_preview          TEXT,
    output_text             TEXT,
    output_hash             TEXT,
    asserts                 TEXT,
    prompt_tokens           BIGINT,
    completion_tokens       BIGINT,
    cost_microusd           BIGINT,
    latency_ms              BIGINT,
    detail                  BYTEA,
    provider_id             TEXT,
    prompt_id               TEXT,
    test_id                 TEXT,
    repeat_idx              BIGINT,
    score                   DOUBLE PRECISION,
    stop_reason             TEXT,
    cached                  BIGINT,
    error                   TEXT,
    prompt_digest           TEXT,
    provider_digest         TEXT,
    assert_digest           TEXT,
    error_class             TEXT,
    cache_key               TEXT,
    empty_reason            TEXT,
    answered_by_provider_id TEXT,
    PRIMARY KEY (run_id, case_key)
);
CREATE INDEX idx_cases_run_status ON cases(run_id, status);
CREATE INDEX idx_cases_run_provider ON cases(run_id, provider_id);
CREATE INDEX idx_cases_run_test ON cases(run_id, test_id);
CREATE INDEX idx_cases_key ON cases(case_key);
CREATE INDEX idx_cases_run_error_class ON cases(run_id, error_class);
-- Partial: `cache_key = ?` implies IS NOT NULL, and the NULL population
-- (every pre-promotion row) would otherwise be carried forever for nothing.
-- No COALESCE on either side, or the lookup stops matching.
CREATE INDEX idx_cases_cache_key ON cases(cache_key)
    WHERE cache_key IS NOT NULL;
CREATE INDEX idx_cases_run_empty_reason ON cases(run_id, empty_reason);

CREATE TABLE case_tags (
    run_id   TEXT NOT NULL,
    case_key TEXT NOT NULL,
    tag      TEXT NOT NULL,
    PRIMARY KEY (run_id, case_key, tag),
    FOREIGN KEY (run_id, case_key) REFERENCES cases(run_id, case_key) ON DELETE CASCADE
);
CREATE INDEX idx_case_tags_tag ON case_tags(run_id, tag);

-- The PK is the `ON CONFLICT(project, suite)` target in projects.rs.
CREATE TABLE baselines (
    project TEXT NOT NULL,
    suite   TEXT NOT NULL,
    run_id  TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    set_at  BIGINT NOT NULL,
    PRIMARY KEY (project, suite)
);

-- users: the SQLite migration-14 rebuild's shape (migration 2 plus
-- migration 4's email, with the viewer role). SSO-only users store '' as
-- password_hash; email stays NULL for local accounts.
CREATE TABLE users (
    id            TEXT PRIMARY KEY,
    username      TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin','member','viewer')),
    disabled      BIGINT NOT NULL DEFAULT 0,
    created_at    BIGINT NOT NULL,
    email         TEXT
);

CREATE TABLE sessions (
    token_hash   TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   BIGINT,
    expires_at   BIGINT,
    last_used_at BIGINT
);
CREATE INDEX idx_sessions_user ON sessions(user_id);

CREATE TABLE api_keys (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT,
    prefix       TEXT NOT NULL,
    key_hash     TEXT NOT NULL,
    scope        TEXT NOT NULL,
    created_at   BIGINT,
    last_used_at BIGINT,
    revoked      BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX idx_api_keys_prefix ON api_keys(prefix);
CREATE INDEX idx_api_keys_user ON api_keys(user_id);

-- SSO (SQLite migration 4). Identities match strictly on provider+subject,
-- never by email — the UNIQUE encodes that.
CREATE TABLE user_identities (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider      TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('oidc','saml')),
    subject       TEXT NOT NULL,
    email         TEXT,
    display_name  TEXT,
    created_at    BIGINT NOT NULL,
    last_login_at BIGINT,
    UNIQUE (provider, subject)
);
CREATE INDEX idx_user_identities_user ON user_identities(user_id);

CREATE TABLE login_transactions (
    id_hash       TEXT PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('oidc','saml')),
    provider      TEXT NOT NULL,
    nonce         TEXT,
    pkce_verifier TEXT,
    request_id    TEXT,
    return_to     TEXT,
    created_at    BIGINT NOT NULL,
    expires_at    BIGINT NOT NULL
);
CREATE INDEX idx_login_txn_expires ON login_transactions(expires_at);

CREATE TABLE saml_consumed_assertions (
    assertion_id    TEXT PRIMARY KEY,
    provider        TEXT NOT NULL,
    not_on_or_after BIGINT NOT NULL
);

-- Run-set policy (SQLite migration 14). NULL suite = the whole project, and
-- a plain UNIQUE cannot encode that (every NULL is distinct), hence the
-- COALESCE expression indexes — a suite may never be named '', so the
-- collapse is lossless. They are the exact ON CONFLICT targets in runsets.rs.
CREATE TABLE run_set_restrictions (
    project    TEXT NOT NULL,
    suite      TEXT,
    created_at BIGINT NOT NULL,
    created_by TEXT
);
CREATE UNIQUE INDEX idx_run_set_restrictions_scope
    ON run_set_restrictions(project, COALESCE(suite,''));

CREATE TABLE run_set_grants (
    id         TEXT PRIMARY KEY,
    project    TEXT NOT NULL,
    suite      TEXT,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    level      TEXT NOT NULL CHECK (level IN ('view','upload','manage')),
    created_at BIGINT NOT NULL,
    created_by TEXT
);
CREATE UNIQUE INDEX idx_run_set_grants_scope
    ON run_set_grants(project, COALESCE(suite,''), user_id);
CREATE INDEX idx_run_set_grants_user ON run_set_grants(user_id);

-- Full-text mirrors (SQLite migration 5's FTS5 tables, as plain tables with
-- generated tsvectors). Column names/positions accept search.rs's shared
-- INSERTs unchanged; ingest writes rows in the same transaction as runs/cases,
-- and delete_run's explicit FTS deletes stay correct (the FK cascade merely
-- makes them redundant here). coalesce + || rather than concat_ws: concat_ws
-- is only STABLE in Postgres, and generated columns require IMMUTABLE.
CREATE TABLE runs_fts (
    run_id      TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    project     TEXT,
    suite       TEXT,
    branch      TEXT,
    commit_sha  TEXT,
    description TEXT,
    tags        TEXT,
    tsv tsvector GENERATED ALWAYS AS (to_tsvector('simple',
        coalesce(project,'') || ' ' || coalesce(suite,'') || ' '
        || coalesce(branch,'') || ' ' || coalesce(commit_sha,'') || ' '
        || coalesce(description,'') || ' ' || coalesce(tags,''))) STORED
);
CREATE INDEX idx_runs_fts_tsv ON runs_fts USING GIN (tsv);

-- Deliberately NO primary key: ingest inserts cases with ON CONFLICT DO
-- NOTHING but calls index_case unconditionally, so an upload carrying a
-- duplicate case_key writes two FTS rows on SQLite (FTS5 has no uniqueness)
-- and must not fail the whole ingest here. The plain index serves the
-- (run_id, case_key) join in case_hits and the run-scoped deletes.
-- FK on run_id only, not the composite to cases: matching FTS5's behavior,
-- which never required a cases row.
CREATE TABLE cases_fts (
    run_id   TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    case_key TEXT NOT NULL,
    name     TEXT,
    prompt   TEXT,
    output   TEXT,
    error    TEXT,
    tags     TEXT,
    tsv tsvector GENERATED ALWAYS AS (to_tsvector('simple',
        coalesce(name,'') || ' ' || coalesce(prompt,'') || ' '
        || coalesce(output,'') || ' ' || coalesce(error,'') || ' '
        || coalesce(tags,''))) STORED
);
CREATE INDEX idx_cases_fts_run_case ON cases_fts(run_id, case_key);
CREATE INDEX idx_cases_fts_tsv ON cases_fts USING GIN (tsv);
"#;

/// Final state of SQLite cache migrations 1–3, as one snapshot.
const CACHE_V1: &str = r#"
-- cache_entries: SQLite migration 1 plus the promoted columns from 2 and 3.
-- body stays opaque by contract (a PUT this server cannot parse must still
-- succeed), so every promoted column is nullable and NULL means "no value".
-- indexed_at/reindexed_at are the two independent backfill progress records
-- (NULL = that pass has not looked yet); index_ok records whether looking
-- found a parseable CacheEntry.
--
-- `id` is Postgres-only: the stable row identity that replaces SQLite's
-- rowid for cacheindex's batch cursors and the FTS join ({id} formats to
-- "rowid" on SQLite, "id" here). `key` stays the PRIMARY KEY so the shared
-- `ON CONFLICT (key) DO NOTHING RETURNING {id}` in cache_put works verbatim.
CREATE TABLE cache_entries (
    key              TEXT PRIMARY KEY,
    id               BIGINT GENERATED ALWAYS AS IDENTITY NOT NULL UNIQUE,
    body             BYTEA NOT NULL,
    size             BIGINT NOT NULL,
    created_at       BIGINT NOT NULL,
    last_access_at   BIGINT NOT NULL,
    indexed_at       BIGINT,
    index_ok         BIGINT,
    kind             TEXT,
    model            TEXT,
    cost_microusd    BIGINT,
    input_tokens     BIGINT,
    output_tokens    BIGINT,
    -- When the *call* happened, as the entry records it; created_at is when
    -- this server was handed the blob.
    entry_created_at BIGINT,
    request_summary  TEXT,
    output_preview   TEXT,
    empty_reason     TEXT,
    reindexed_at     BIGINT
);
CREATE INDEX idx_cache_last_access   ON cache_entries(last_access_at);
CREATE INDEX idx_cache_created       ON cache_entries(created_at);
CREATE INDEX idx_cache_size          ON cache_entries(size);
CREATE INDEX idx_cache_kind_created  ON cache_entries(kind, created_at);
CREATE INDEX idx_cache_model_created ON cache_entries(model, created_at);
-- Partial indexes, each matching its query's predicate exactly (no COALESCE
-- on either side or the planner stops matching). The two backfill probes
-- drain to empty, which is the steady state; the cost sort excludes NULL
-- cost outright; empty_reason indexes only the rows anyone searches for.
CREATE INDEX idx_cache_unindexed ON cache_entries(indexed_at)
    WHERE indexed_at IS NULL;
CREATE INDEX idx_cache_cost ON cache_entries(cost_microusd, key)
    WHERE cost_microusd IS NOT NULL;
CREATE INDEX idx_cache_empty_reason ON cache_entries(empty_reason)
    WHERE empty_reason IS NOT NULL;
CREATE INDEX idx_cache_unreindexed ON cache_entries(reindexed_at)
    WHERE reindexed_at IS NULL;

CREATE TABLE cache_counters (
    id     BIGINT PRIMARY KEY CHECK (id = 1),
    hits   BIGINT NOT NULL DEFAULT 0,
    misses BIGINT NOT NULL DEFAULT 0
);
-- Seed the singleton like SQLite's migration 1; idempotent so a re-run of a
-- partially-applied v1 cannot fail here.
INSERT INTO cache_counters (id, hits, misses) VALUES (1, 0, 0)
    ON CONFLICT (id) DO NOTHING;

-- FTS mirror of SQLite migration 2's cache_entries_fts, keyed by the entry
-- id. The FK cascade replaces the cache_entries_delete_fts trigger, and the
-- PK is safe because insert_fts is delete-then-insert. Request text is
-- weighted above output so an entry whose *request* mentions a term ranks
-- ahead of one whose response merely discusses it.
CREATE TABLE cache_entries_fts (
    id      BIGINT PRIMARY KEY REFERENCES cache_entries(id) ON DELETE CASCADE,
    request TEXT,
    output  TEXT,
    tsv tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', coalesce(request,'')), 'A')
        || to_tsvector('simple', coalesce(output,''))) STORED
);
CREATE INDEX idx_cache_fts_tsv ON cache_entries_fts USING GIN (tsv);
"#;
