//! Forward-only migration SQL for both databases.

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

/// Bring `conn` to the latest runs schema, with foreign keys disabled for the
/// duration.
///
/// Migration 14 rebuilds `users`, and SQLite only permits a table rebuild with
/// enforcement off: `DROP TABLE` runs an implicit `DELETE FROM` that fires
/// every `ON DELETE CASCADE` hanging off the table, and `ALTER TABLE … RENAME`
/// rewrites the children's `REFERENCES` clauses to name the scratch table. The
/// pragma is a no-op inside a transaction and rusqlite_migration runs each
/// migration in one, so it has to be toggled out here, on the connection,
/// around the whole run — this is step 1 of SQLite's own documented rebuild
/// recipe.
///
/// **The pragma is off for the whole batch, so *every* migration runs without
/// foreign-key enforcement — including every one added after this comment.** A
/// migration that inserts or backfills rows cannot rely on a `REFERENCES`
/// clause to reject a dangling parent; check the invariant in SQL, or verify it
/// with `PRAGMA foreign_key_check` before the batch ends.
pub(super) fn migrate_runs(conn: &mut Connection) -> anyhow::Result<()> {
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let applied = runs_migrations().to_latest(conn);
    // Restore enforcement whatever happened: the caller keeps this connection.
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(applied?)
}

/// Schema for `domarinn.db` (durable run history).
///
/// Migrations are forward-only and append-only: never edit an existing `M::up`,
/// only add new ones to the end of the vec so existing databases upgrade in
/// place. The accounts tables (migration 2) live in the runs database so they
/// share its writer-mutex and reader-pool.
fn runs_migrations() -> Migrations<'static> {
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
        // Migration 4: SSO. `user_identities` links a user to an IdP identity
        // (matched strictly on provider+subject — never by email);
        // `login_transactions` holds in-flight OIDC state/PKCE and SAML
        // request ids (single-use, short TTL); `saml_consumed_assertions` is
        // the assertion-replay cache. SSO-only users store `''` as their
        // `password_hash` (login rejects it; avoids a table rebuild to drop
        // NOT NULL). `users.email` stays NULL for local accounts.
        M::up(
            r#"
        ALTER TABLE users ADD COLUMN email TEXT;

        CREATE TABLE user_identities (
            id            TEXT PRIMARY KEY,
            user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            provider      TEXT NOT NULL,
            kind          TEXT NOT NULL CHECK (kind IN ('oidc','saml')),
            subject       TEXT NOT NULL,
            email         TEXT,
            display_name  TEXT,
            created_at    INTEGER NOT NULL,
            last_login_at INTEGER,
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
            created_at    INTEGER NOT NULL,
            expires_at    INTEGER NOT NULL
        );
        CREATE INDEX idx_login_txn_expires ON login_transactions(expires_at);

        CREATE TABLE saml_consumed_assertions (
            assertion_id    TEXT PRIMARY KEY,
            provider        TEXT NOT NULL,
            not_on_or_after INTEGER NOT NULL
        );
        "#,
        ),
        // Full-text search. Plain FTS5 tables (not external-content: prompt and
        // error text exist only inside the compressed `cases.detail` blobs, so
        // no content table has them). Ingest writes these rows in the same
        // transaction as `cases`/`runs`; `delete_run` removes them explicitly
        // (FTS tables have no FK cascade); the startup backfill indexes rows
        // that predate this migration (`storage::search::backfill`).
        M::up(
            r#"
        CREATE VIRTUAL TABLE runs_fts USING fts5(
            run_id UNINDEXED, project, suite, branch, commit_sha, description, tags
        );
        CREATE VIRTUAL TABLE cases_fts USING fts5(
            run_id UNINDEXED, case_key UNINDEXED, name, prompt, output, error, tags
        );
        "#,
        ),
        // Migration 6: promote the cache facts out of the zstd blobs so the
        // list endpoints can filter CI cache-noise at SQL level
        // (`RunSummary.cache_hits/cache_misses` → `runs`, `CaseResult.cached`
        // → `cases`). Ingest writes them going forward; `backfill` populates
        // pre-existing rows on open. NULL = unknown (pre-backfill legacy);
        // -1 = undecodable-blob sentinel (numeric cousin of migration 3's
        // empty-string sentinel). Both must never be hidden by the filters.
        M::up(
            r#"
        ALTER TABLE runs ADD COLUMN cache_hits INTEGER;
        ALTER TABLE runs ADD COLUMN cache_misses INTEGER;
        ALTER TABLE cases ADD COLUMN cached INTEGER;
        "#,
        ),
        // Migration 7: promote the failure reason out of the zstd blob.
        // `output_preview` is derived from `output`, and an errored case has no
        // output — so the preview is NULL for exactly the rows a reader most
        // wants to skim, and every error row had to be opened individually to
        // learn anything. Ingest writes it going forward; `backfill` populates
        // pre-existing rows on open.
        //
        // Tri-state, following migration 3's empty-string sentinel: NULL =
        // not yet backfilled (unknown), '' = known to have no error, non-empty
        // = the message. It cannot be a plain nullable column, because NULL is
        // also the honest value for every passing case and the backfill
        // predicate would then rescan the whole table forever.
        M::up(
            r#"
        ALTER TABLE cases ADD COLUMN error TEXT;
        "#,
        ),
        // Migration 8: run provenance columns, plus the indexes the
        // origin/actor/branch facets need.
        //
        // Unlike migrations 3/6/7 this needs **no backfill and no sentinel**.
        // Those promoted facts that already existed inside the blobs, so NULL
        // was ambiguous ("not yet extracted" vs "genuinely absent") and had to
        // be disambiguated. `RunOrigin` is new: no run written before it can
        // carry one, so NULL here means exactly "the client did not record an
        // actor" and always will. Adding a backfill pass would decompress every
        // historical blob to discover nothing.
        //
        // `git_dirty`/`ci_provider` are deliberately not re-added — they have
        // been columns since migration 1 and were simply never surfaced.
        M::up(
            r#"
        ALTER TABLE runs ADD COLUMN actor TEXT;
        ALTER TABLE runs ADD COLUMN host TEXT;
        ALTER TABLE runs ADD COLUMN domarinn_version TEXT;

        CREATE INDEX idx_runs_actor ON runs(actor, created_at DESC);
        CREATE INDEX idx_runs_ci ON runs(ci_provider, created_at DESC);
        CREATE INDEX idx_runs_uploaded_by ON runs(uploaded_by, created_at DESC);
        -- Not a prefix of idx_runs_proj_suite_created: that one still serves
        -- branch-agnostic ordering without a sort, so both earn their keep.
        CREATE INDEX idx_runs_proj_suite_branch_created
            ON runs(project, suite, git_branch, created_at DESC);
        "#,
        ),
        // Migration 9: component digests, promoted so a comparison can say
        // *what* changed without decompressing two blobs, and the runs list can
        // say it from the row.
        //
        // No backfill, and the reason differs per column — worth stating
        // precisely, because two of these ARE recoverable and we are choosing
        // not to:
        //
        // - `cases.provider_digest` is genuinely unrecoverable: the provider
        //   fingerprint appears nowhere in a stored run document (`request` is
        //   a preview envelope, absent under `--no-raw`).
        // - `cases.assert_digest` is recomputable from stored `criteria`, but
        //   buys nothing alone: `classify_change` reports Unknown unless every
        //   axis is known, and `provider_digest` never will be.
        // - The five run-level digests over `prompts`/`providers`/`grader` ARE
        //   recoverable from `config_snapshot` in each blob, and `ComponentDrift`
        //   treats each component independently, so backfilling them WOULD light
        //   up the change chips for historical comparisons. We skip it anyway:
        //   the pass is O(runs) with a zstd decompress each, inside
        //   `open_blocking` before the server accepts traffic. The cost is that
        //   comparisons against pre-deploy runs report `changed: null` — which
        //   every reader already renders as unknown rather than unchanged.
        //
        // The consequence is a contract, not an accident: change classification
        // only works between two runs that both post-date this migration, and
        // every reader must render that as unknown rather than unchanged.
        M::up(
            r#"
        ALTER TABLE runs ADD COLUMN prompts_digest   TEXT;
        ALTER TABLE runs ADD COLUMN providers_digest TEXT;
        ALTER TABLE runs ADD COLUMN tests_digest     TEXT;
        ALTER TABLE runs ADD COLUMN asserts_digest   TEXT;
        ALTER TABLE runs ADD COLUMN grader_digest    TEXT;

        ALTER TABLE cases ADD COLUMN prompt_digest   TEXT;
        ALTER TABLE cases ADD COLUMN provider_digest TEXT;
        ALTER TABLE cases ADD COLUMN assert_digest   TEXT;
        "#,
        ),
        // Migration 10: promote the structured failure class, so a run's errors
        // can be aggregated instead of read one case at a time. `provider_*` is
        // "not your model's fault", `grader_*` is "the eval did not run" — a UI
        // that renders both as "14 errors" makes you open fourteen cases to
        // learn that none of them were about the model.
        //
        // NULL is unambiguous here (no run written before this carries a class)
        // and therefore needs no sentinel — unlike migration 7's `error`, where
        // NULL was also the honest value for every passing case.
        M::up(
            r#"
        ALTER TABLE cases ADD COLUMN error_class TEXT;
        CREATE INDEX idx_cases_run_error_class ON cases(run_id, error_class);
        "#,
        ),
        // Migration 11: index the hidden-cached-runs predicate.
        //
        // The runs list defaults to `cached=exclude`, and on every first page
        // that computes `COUNT(*)` over the whole `runs` table with a predicate
        // no index could serve. It is the hottest read in the product and it was
        // O(rows) unbounded, against a reader pool of four.
        //
        // A partial index matching the predicate exactly. SQLite will only use
        // one when the query's WHERE provably implies the index's, which is why
        // `FULLY_CACHED` had to drop its `COALESCE` wrapper first — the two
        // forms are semantically identical, but a function call around a column
        // blocks the match.
        M::up(
            r#"
        CREATE INDEX idx_runs_cached_passing ON runs(created_at DESC)
        WHERE cache_misses = 0 AND cache_hits > 0
          AND fail_count = 0 AND error_count = 0;
        "#,
        ),
        // Migration 12: the cache-token and second-cost counters, promoted
        // because the run header now renders them and the alternative is
        // decompressing a whole run blob to draw four numbers.
        //
        // These were deliberately left in the blob when they were added — the
        // repo's rule is to promote when a *query* needs a fact, not when one
        // exists. A query needs them now.
        //
        // No backfill, and NULL is load-bearing rather than zero. A run stored
        // before this migration may well have read from a prompt cache; we
        // simply did not record it in a column, and writing `0` would assert it
        // did not. Every reader renders NULL as absent, which is what the CLI
        // already does for the same numbers.
        //
        // `cache_savings_microusd` and `grader_cost_microusd` follow
        // `cost_microusd`'s integer storage: money is summed, and a float
        // column would reintroduce exactly the order-dependent total the
        // engine's integer accumulator exists to prevent.
        M::up(
            r#"
        ALTER TABLE runs ADD COLUMN cache_read_tokens      INTEGER;
        ALTER TABLE runs ADD COLUMN cache_write_tokens     INTEGER;
        ALTER TABLE runs ADD COLUMN cache_savings_microusd INTEGER;
        ALTER TABLE runs ADD COLUMN grader_cost_microusd   INTEGER;
        "#,
        ),
        // Migration 13: promote the provider call's cache key, so a cached case
        // links to the entry that answered it and an entry can name the runs
        // that used it.
        //
        // No backfill, and no sentinel — migration 8's reason rather than
        // migration 7's. `CaseResult.cache_key` is *new*: no run recorded
        // before it can carry one, so NULL here means exactly "this case had no
        // key" and always will.
        //
        // Worth being precise that a backfill is not merely expensive but
        // impossible. The key is
        // `request_cache_key(canonical_request, repeat, provider_salt,
        // case_salt)`, and a stored run document holds none of those three
        // ingredients: `CaseResult.request` is a *preview* envelope, not the
        // canonical request (`exec`'s canonical strips `test`; `http` declines
        // a preview outright to avoid persisting env-templated credentials;
        // `--no-raw` drops it), and neither salt appears anywhere in the
        // document. A backfill would produce wrong keys, not missing ones.
        //
        // So this is a contract, exactly as migration 9's is: cross-linking
        // works between an entry and a run produced by this version or later,
        // and every reader must render absence as unknown rather than as "no
        // cache was used".
        M::up(
            r#"
        ALTER TABLE cases ADD COLUMN cache_key TEXT;

        -- Partial, matching the lookup's predicate exactly. `cache_key = ?1`
        -- provably implies `cache_key IS NOT NULL`, so SQLite will use it — and
        -- the NULL population here is every row ever written before this
        -- migration, which a full index would carry forever for nothing. See
        -- migration 11 for the matching rule, and for why no COALESCE may
        -- appear on either side.
        CREATE INDEX idx_cases_cache_key ON cases(cache_key)
            WHERE cache_key IS NOT NULL;
        "#,
        ),
        // Migration 14: the read-only `viewer` role, and the two tables run-set
        // visibility will be decided from.
        //
        // Widening a CHECK means rebuilding the table — SQLite has no ALTER for
        // it — and `users` is the one table here with children hanging off
        // `ON DELETE CASCADE` (`sessions`, `api_keys`, `user_identities`). The
        // rebuild below is SQLite's documented recipe, and it is only safe
        // because [`migrate_runs`] turns foreign keys off around the whole
        // migration run: with enforcement on, the `DROP TABLE` fires every
        // cascade (every session, API key and SSO link on the instance, gone on
        // the next restart) and the `RENAME` rewrites the children's
        // `REFERENCES` clauses to name `users_new`. See that function for why
        // the pragma cannot live in this SQL.
        //
        // The column set is migration 2's plus migration 4's `email`.
        // `username` carries its own UNIQUE constraint (an implicit index,
        // recreated with the table); migration 2 declared no explicit index on
        // `users`, so there is none to recreate here.
        //
        // The two new tables are inert — nothing reads or writes them yet. A
        // NULL `suite` means "the whole project" in both, and the uniqueness
        // that encodes cannot be a plain UNIQUE constraint: SQLite treats every
        // NULL as distinct, so `(project, NULL)` would insert twice over and a
        // project-wide rule would exist in N conflicting copies. Hence the
        // expression indexes over `COALESCE(suite,'')` — a suite may never be
        // named `''`, so the collapse is lossless. `created_at` is epoch-ms
        // like every other timestamp here; `created_by` is a free label, and
        // NULL means the row records no author.
        M::up(
            r#"
        CREATE TABLE users_new (
            id            TEXT PRIMARY KEY,
            username      TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            role          TEXT NOT NULL CHECK (role IN ('admin','member','viewer')),
            disabled      INTEGER NOT NULL DEFAULT 0,
            created_at    INTEGER NOT NULL,
            email         TEXT
        );
        INSERT INTO users_new (id, username, password_hash, role, disabled, created_at, email)
            SELECT id, username, password_hash, role, disabled, created_at, email FROM users;
        DROP TABLE users;
        ALTER TABLE users_new RENAME TO users;

        CREATE TABLE run_set_restrictions (
            project    TEXT NOT NULL,
            suite      TEXT,
            created_at INTEGER NOT NULL,
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
            created_at INTEGER NOT NULL,
            created_by TEXT
        );
        CREATE UNIQUE INDEX idx_run_set_grants_scope
            ON run_set_grants(project, COALESCE(suite,''), user_id);
        CREATE INDEX idx_run_set_grants_user ON run_set_grants(user_id);
        "#,
        ),
        // Migration 15: promote why an output came back empty, and how many of a
        // run's cases did. An empty output is a *successful* provider call, so a
        // suite that quietly went all-empty looks like a suite that quietly
        // started failing — the reason is the difference between "raise
        // max_tokens" and "the model now refuses this prompt", and it was only
        // readable one decompressed case at a time.
        //
        // `empty_reason` is a tri-state exactly like migration 7's `error`, and
        // for the same reason: NULL is the honest value for the majority of
        // cases (they had output), so it cannot also mean "not yet backfilled".
        // NULL = not backfilled, `''` = decoded and there was no reason,
        // non-empty = the reason. Ingest must therefore write `''`, never NULL.
        //
        // `empty_count` is the run-level tally, with -1 as the undecodable-blob
        // sentinel (migration 6's numeric convention). A blob written before
        // `empty_reason` existed honestly counts 0: the field is absent from the
        // document, so no case in it carried one.
        M::up(
            r#"
        ALTER TABLE cases ADD COLUMN empty_reason TEXT;
        CREATE INDEX idx_cases_run_empty_reason ON cases(run_id, empty_reason);
        ALTER TABLE runs ADD COLUMN empty_count INTEGER;
        "#,
        ),
        // Migration 16: hiding a replay no longer depends on its verdict, so
        // the index that served the old predicate is replaced by one matching
        // the new one.
        //
        // Migration 11's index carried `AND fail_count = 0 AND error_count = 0`
        // because "hidden" then meant fully cached *and* passing. That rule
        // assumed a failure in a replay is rare and therefore interesting. In a
        // suite with a stable failing subset — a known-failing set of cases that
        // fails identically on every replay — it fires on every run instead, so
        // the filter suppresses nothing and the feature does not work at all.
        //
        // SQLite only uses a partial index when the query's WHERE provably
        // implies the index's, and `cache_misses = 0 AND cache_hits > 0` does
        // not imply the verdict terms. The old index would therefore go unused
        // while still costing a write on every insert, so it goes.
        M::up(
            r#"
        DROP INDEX IF EXISTS idx_runs_cached_passing;
        CREATE INDEX idx_runs_fully_cached ON runs(created_at DESC)
        WHERE cache_misses = 0 AND cache_hits > 0;
        "#,
        ),
    ])
}

/// Schema for `cache.db` (disposable content-addressed cache).
pub(super) fn cache_migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
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
        ),
        // Migration 2: promote the browsable facts out of the opaque `body`
        // blob, and index the searchable text, so entries can be listed,
        // filtered and searched without decoding every row.
        //
        // `body` stays opaque *by contract* — the cache is a blob store and a
        // PUT this server cannot parse must still succeed (see `cache_put`) —
        // so every promoted column is nullable and NULL means "no value",
        // never zero. Migration 12's argument on the runs side, applied here.
        //
        // `indexed_at` / `index_ok` are the tri-state that makes the backfill
        // terminate: NULL = never looked at, non-NULL = looked at, and
        // `index_ok` records whether looking found a `CacheEntry`. This is
        // migration 7's sentinel lesson expressed as one state column rather
        // than one sentinel per promoted column — there are eight promoted
        // columns here, and eight sentinels would be eight chances to leave a
        // row eligible for every future pass.
        M::up(
            r#"
        ALTER TABLE cache_entries ADD COLUMN indexed_at       INTEGER;
        ALTER TABLE cache_entries ADD COLUMN index_ok         INTEGER;
        ALTER TABLE cache_entries ADD COLUMN kind             TEXT;
        ALTER TABLE cache_entries ADD COLUMN model            TEXT;
        ALTER TABLE cache_entries ADD COLUMN cost_microusd    INTEGER;
        ALTER TABLE cache_entries ADD COLUMN input_tokens     INTEGER;
        ALTER TABLE cache_entries ADD COLUMN output_tokens    INTEGER;
        -- When the *call* happened, as the entry records it. Distinct from
        -- `created_at`, which is when this server was handed the blob — for a
        -- team tier those differ by however long the entry sat on a laptop.
        ALTER TABLE cache_entries ADD COLUMN entry_created_at INTEGER;
        ALTER TABLE cache_entries ADD COLUMN request_summary  TEXT;
        ALTER TABLE cache_entries ADD COLUMN output_preview   TEXT;

        CREATE INDEX idx_cache_created       ON cache_entries(created_at);
        CREATE INDEX idx_cache_size          ON cache_entries(size);
        CREATE INDEX idx_cache_kind_created  ON cache_entries(kind, created_at);
        CREATE INDEX idx_cache_model_created ON cache_entries(model, created_at);

        -- Partial, matching each query's predicate exactly. The backfill probe
        -- runs against an index that empties as it drains, and the cost sort
        -- excludes NULL cost outright rather than wrapping the column in a
        -- COALESCE that would make it unusable. See migration 11.
        CREATE INDEX idx_cache_unindexed ON cache_entries(indexed_at)
            WHERE indexed_at IS NULL;
        CREATE INDEX idx_cache_cost ON cache_entries(cost_microusd, key)
            WHERE cost_microusd IS NOT NULL;

        -- Plain fts5, not external-content: an entry's *body* is immutable, so
        -- the indexed text never changes for a given row and there is nothing
        -- to gain from making the blob table the content source. The index
        -- itself is nevertheless rewritable — migration 3 re-derives promoted
        -- columns for rows indexed by an older build, and `insert_fts` is
        -- delete-then-insert for exactly that reason (see `cacheindex`).
        -- Rowids are kept aligned with `cache_entries` so a delete is a rowid
        -- lookup rather than a scan over an UNINDEXED key column.
        CREATE VIRTUAL TABLE cache_entries_fts USING fts5(request, output);

        -- Deletes are the only mutation, and they arrive from two directions
        -- (`cache_prune`'s age pass and its LRU pass), so the sync belongs in
        -- a trigger rather than at each call site. Inserts cannot be a trigger:
        -- the text comes from parsing a body the trigger cannot see.
        CREATE TRIGGER cache_entries_delete_fts AFTER DELETE ON cache_entries BEGIN
            DELETE FROM cache_entries_fts WHERE rowid = old.rowid;
        END;
        "#,
        ),
        // Migration 3: promote `empty_reason` so the poison a refusal leaves
        // behind can be *found*. Without it the only cure for one bad draw
        // frozen into a shared cache is `older_than_days=0`, which throws away
        // the warm cache to evict a handful of rows.
        //
        // `reindexed_at` is the same tri-state trick migration 2 used for
        // `indexed_at`, one turn further on: migration 2's rows were indexed by
        // a build that had never heard of `empty_reason`, so they are stamped
        // "looked at" and would otherwise never be looked at again. NULL means
        // "indexed by a pre-migration-3 build"; the background pass in
        // `cacheindex` fills it in.
        //
        // Deliberately NOT `UPDATE cache_entries SET indexed_at = NULL`.
        // Migrations run inside `Storage::open_blocking`, *before the port
        // binds*, and a full-table UPDATE rewriting pages that hold multi-MiB
        // bodies is exactly the "a restart becomes an outage" property the
        // background backfill exists to avoid (see `cacheindex`'s module doc).
        // An ALTER that appends a NULL column is O(1) in SQLite; a rewrite is
        // not.
        M::up(
            r#"
        ALTER TABLE cache_entries ADD COLUMN empty_reason TEXT;
        ALTER TABLE cache_entries ADD COLUMN reindexed_at INTEGER;

        -- Partial, matching the filter and the facet query exactly: the
        -- overwhelming majority of entries have no reason at all, and indexing
        -- their NULLs would be paying for the rows nobody searches for.
        CREATE INDEX idx_cache_empty_reason ON cache_entries(empty_reason)
            WHERE empty_reason IS NOT NULL;

        -- Drains to empty, like `idx_cache_unindexed`. Rows this server could
        -- not parse are stamped without re-reading their bodies rather than
        -- left pending: re-parsing a body that already failed cannot succeed,
        -- and leaving them in the index makes every future pass scan them.
        CREATE INDEX idx_cache_unreindexed ON cache_entries(reindexed_at)
            WHERE reindexed_at IS NULL;
        "#,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    /// A runs database stopped at migration 13, with foreign keys enforced
    /// exactly as `open_conn` configures the real writer connection — the
    /// state every deployed instance upgrades into migration 14 from.
    fn v13_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        runs_migrations().to_version(&mut conn, 13).unwrap();
        conn
    }

    /// A runs database at the latest migration, reached through the same
    /// [`migrate_runs`] the server's own open path uses.
    fn latest_conn() -> Connection {
        let mut conn = v13_conn();
        migrate_runs(&mut conn).unwrap();
        conn
    }

    fn seed_user(conn: &Connection, id: &str, username: &str, role: &str) {
        conn.execute(
            "INSERT INTO users (id, username, password_hash, role, disabled, created_at, email)
             VALUES (?1, ?2, 'phc', ?3, 0, 1700000000000, ?4)",
            params![id, username, role, format!("{username}@example.com")],
        )
        .unwrap();
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// The `users` rebuild is the risky half of migration 14: SQLite cannot
    /// widen a CHECK in place, and a naive rebuild drops every session, API
    /// key and SSO identity on the floor via `ON DELETE CASCADE`.
    #[test]
    fn migration_14_rebuilds_users_without_losing_rows_or_their_children() {
        let mut conn = v13_conn();
        seed_user(&conn, "u1", "root", "admin");
        seed_user(&conn, "u2", "dana", "member");
        conn.execute_batch(
            "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_used_at)
                 VALUES ('sh', 'u2', 1, 2, 3);
             INSERT INTO api_keys (id, user_id, name, prefix, key_hash, scope, created_at)
                 VALUES ('k1', 'u2', 'ci', 'domarinn_ab', 'kh', 'write', 1);
             INSERT INTO user_identities
                 (id, user_id, provider, kind, subject, email, display_name, created_at)
                 VALUES ('i1', 'u2', 'oidc:corp', 'oidc', 'sub-1', 'd@example.com', 'Dana', 1);",
        )
        .unwrap();

        migrate_runs(&mut conn).unwrap();

        // Every user column survives the rebuild, values included.
        let (username, role, email, disabled, created_at): (String, String, String, i64, i64) =
            conn.query_row(
                "SELECT username, role, email, disabled, created_at FROM users WHERE id = 'u2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(username, "dana");
        assert_eq!(role, "member");
        assert_eq!(email, "dana@example.com");
        assert_eq!(disabled, 0);
        assert_eq!(created_at, 1_700_000_000_000);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM users"), 2);

        // The children the FKs point at are still there.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM sessions"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM api_keys"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM user_identities"), 1);

        // `username` is still unique.
        assert!(conn
            .execute(
                "INSERT INTO users (id, username, password_hash, role, disabled, created_at)
                 VALUES ('u3', 'dana', 'phc', 'member', 0, 1)",
                [],
            )
            .is_err());

        // And the cascade still fires, so the FKs point at the new table.
        conn.execute("DELETE FROM users WHERE id = 'u2'", [])
            .unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM sessions"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM api_keys"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM user_identities"), 0);
    }

    #[test]
    fn migration_14_admits_viewer_and_still_rejects_unknown_roles() {
        let conn = latest_conn();
        seed_user(&conn, "u1", "reader", "viewer");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM users WHERE role = 'viewer'"),
            1
        );
        assert!(conn
            .execute(
                "INSERT INTO users (id, username, password_hash, role, disabled, created_at)
                 VALUES ('u2', 'sneaky', 'phc', 'superadmin', 0, 1)",
                [],
            )
            .is_err());
    }

    /// A project-level restriction is one row, not many: plain `UNIQUE` would
    /// treat every NULL suite as distinct and let duplicates through.
    #[test]
    fn a_project_level_restriction_can_only_be_recorded_once() {
        let conn = latest_conn();
        let insert = |suite: Option<&str>| {
            conn.execute(
                "INSERT INTO run_set_restrictions (project, suite, created_at, created_by)
                 VALUES ('checkout', ?1, 1, 'root')",
                params![suite],
            )
        };
        assert!(insert(None).is_ok());
        assert!(insert(None).is_err(), "NULL suites must collide");
        assert!(insert(Some("smoke")).is_ok());
        assert!(insert(Some("smoke")).is_err());
    }

    #[test]
    fn a_grant_is_unique_per_scope_and_dies_with_its_user() {
        let conn = latest_conn();
        seed_user(&conn, "u1", "reader", "viewer");
        let insert = |id: &str, suite: Option<&str>, level: &str| {
            conn.execute(
                "INSERT INTO run_set_grants
                     (id, project, suite, user_id, level, created_at, created_by)
                 VALUES (?1, 'checkout', ?2, 'u1', ?3, 1, 'root')",
                params![id, suite, level],
            )
        };
        assert!(insert("g1", None, "view").is_ok());
        assert!(
            insert("g2", None, "manage").is_err(),
            "one grant per project/suite/user, whatever its level"
        );
        assert!(insert("g3", Some("smoke"), "upload").is_ok());
        assert!(insert("g4", Some("smoke"), "upload").is_err());
        assert!(
            insert("g5", Some("other"), "owner").is_err(),
            "unknown level"
        );

        conn.execute("DELETE FROM users WHERE id = 'u1'", [])
            .unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM run_set_grants"), 0);
    }
}
