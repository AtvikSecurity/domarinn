# REST API

All endpoints live under `/api/v1` (health is also mirrored at the bare `/health`). Responses are JSON; errors render as `{ "error": "<message>" }` with an appropriate status. The **Scope** column is the route's *minimum* scope — what it demands in `closed` mode, and (for `write`/`admin`) in `protect-writes`. In `open` mode nothing is required.

Request-body size is capped at **64 MiB**; request bodies may be gzip/deflate compressed (the server decompresses transparently).

## Health & meta

| Method | Path              | Scope | Notes |
|--------|-------------------|-------|-------|
| GET    | `/health`         | —     | `{ "status": "ok" }`. Also the container healthcheck target. |
| GET    | `/api/v1/health`  | —     | Same as above. |
| GET    | `/api/v1/meta`    | —     | Server metadata (below). Always open. |

`GET /api/v1/meta` returns:

```json
{
  "name": "domarinn",
  "version": "0.1.0",
  "auth_mode": "protect-writes",
  "setup_required": false,
  "supported_schema_versions": [1, 2],
  "result_schema_version": 2,
  "cache": { "max_entry_bytes": 4194304, "max_bytes": 1073741824, "max_age_days": 30 }
}
```

## Auth (accounts, sessions)

| Method | Path                    | Scope | Notes |
|--------|-------------------------|-------|-------|
| POST   | `/api/v1/auth/setup`    | —     | Create the first admin. Open only while zero users exist, else `409`. Returns a session token. |
| POST   | `/api/v1/auth/login`    | —     | Exchange `{username, password}` for a session token. `401` on bad/disabled credentials. |
| POST   | `/api/v1/auth/logout`   | (authenticated) | Revoke the presenting session + clear the cookie. No-op `200` for token/API-key callers; `401` for anonymous. |
| GET    | `/api/v1/auth/me`       | —     | Report the current identity: `{authenticated, user, source, scope}`. `source` is `anonymous` \| `static` \| `apikey` \| `session`. |

## SSO (only present when configured — see [Single sign-on](./server.md#single-sign-on-oidc--saml))

| Method | Path                    | Scope | Notes |
|--------|-------------------------|-------|-------|
| GET    | `/api/v1/auth/oidc/{provider}/start` | — | Begin an OIDC login; `303` to the IdP. `?return_to=/path` deep-links back. |
| GET    | `/api/v1/auth/oidc/{provider}/callback` | — | OIDC redirect target; `303` home or to `/login?sso_error=…`. |
| GET    | `/api/v1/auth/saml/{provider}/start` | — | Begin a SAML login; `303` to the IdP (redirect binding). |
| POST   | `/api/v1/auth/saml/{provider}/acs` | — | SAML assertion consumer (HTTP-POST binding). |
| GET    | `/api/v1/auth/saml/{provider}/metadata` | — | SP metadata XML for the IdP to import. |

## API keys

These require an **account-backed** identity (session or API key). A static token has no owning user and gets a `403` here.

| Method | Path                    | Scope   | Notes |
|--------|-------------------------|---------|-------|
| GET    | `/api/v1/apikeys`       | `write` | List the caller's own keys (never the secret). |
| POST   | `/api/v1/apikeys`       | `write` | Mint a key: `{name?, scope?}`. Scope defaults to the caller's own and may not exceed it (`403`). Returns the secret **once** as `key`. |
| DELETE | `/api/v1/apikeys/{id}`  | `write` | Revoke a key. Allowed for its owner or any admin, else `403`. |

## Users administration

| Method | Path                    | Scope   | Notes |
|--------|-------------------------|---------|-------|
| GET    | `/api/v1/users`         | `admin` | List all accounts. |
| POST   | `/api/v1/users`         | `admin` | Create an account: `{username, password, role}` (`role` = `admin`\|`member`). |
| PATCH  | `/api/v1/users/{id}`    | `admin` | Update `role`, `disabled`, and/or `password` (any subset). |
| DELETE | `/api/v1/users/{id}`    | `admin` | Delete an account. Refuses the **last admin** (`409`). |

## Runs

| Method | Path                                    | Scope   | Notes |
|--------|-----------------------------------------|---------|-------|
| POST   | `/api/v1/runs`                          | `write` | Ingest a run document. See [ingest](#run-ingest). |
| GET    | `/api/v1/runs`                          | `read`  | List runs (filterable, paginated). Filters: `project`, `suite`, `tag`, `branch`, `since`, `until`, `status`, `cached`, `origin` (`ci`\|`local`), `actor`. |
| GET    | `/api/v1/runs/{id}`                      | `read`  | Full run detail. `404` if unknown. |
| GET    | `/api/v1/runs/{id}/cases`               | `read`  | Lean list of the run's cases (filterable, paginated). |
| GET    | `/api/v1/runs/{id}/cases/{case_key}`    | `read`  | One case's full detail. |
| GET    | `/api/v1/runs/{id}/matrix`              | `read`  | Prompt × provider aggregate matrix (rows = tests, paginated). |
| GET    | `/api/v1/runs/{id}/export`              | `read`  | The original, lossless run document. |
| GET    | `/api/v1/runs/{id}/config`              | `read`  | The run's config digest + snapshot (no full export). |
| GET    | `/api/v1/runs/{id}/compare/{other}`     | `read`  | Diff two runs (regressions/improvements per case). |
| DELETE | `/api/v1/runs/{id}`                      | `admin` | Delete a run. `204` on success. |

`GET /runs/{id}` reports two cost figures, and they are never summed: `cost_usd` is what the systems under test cost, `grader_cost_usd` is what grading them cost. It also carries `cache_read_tokens`, `cache_write_tokens` and `cache_savings_usd`. All four are `null` for runs ingested before the columns existed — which is **not** the same as zero, and readers render it as unknown rather than as "no activity". There is no backfill; see the migration note in `storage/schema.rs` for why.

<a id="run-ingest"></a>**Ingest** (`POST /api/v1/runs`) accepts a `RunResult`
JSON document (see [`protocol.md`](./protocol.md) and `domarinn schema result`). The body must carry a `schema_version` within the supported window (`result_schema_version - 1 ..= result_schema_version`), else `422`. Ingest is **idempotent by content**:

| Status | Meaning |
|--------|---------|
| `201 Created` | New run stored. Body: `{ "id", "url" }`. |
| `200 OK`      | Identical run id + content already existed. Body: `{ "id", "url" }`. |
| `409 Conflict`| Same run id, **different** content. |

The `url` in the response is a browser link to the run. It is built from `DOMARINN_PUBLIC_URL` when set; otherwise from the request's `Host` header and `X-Forwarded-Proto` (see [`../guides/self-host.md`](../guides/self-host.md#reverse-proxies-and-share-links)).

**List filters** (`GET /api/v1/runs`, all optional query params): `project`, `suite`, `tag`, `branch`, `status`, `since`, `until` (each epoch-ms *or* RFC3339), `limit` (default `50`, max `200`), `cursor`. The response is `{ "runs": [...], "next_cursor": "<cursor|null>" }`; pass `next_cursor` back as `cursor` to page.

**Case filters** (`GET /api/v1/runs/{id}/cases`): `status`, `tag`, `q` (free-text), `provider`, `prompt`, `test`, `stop_reason` (each an exact match on the promoted cell columns), `limit`, `cursor`.

**Matrix** (`GET /api/v1/runs/{id}/matrix`) returns the run's prompt × provider aggregate. `columns` is the complete set of `(provider, prompt)` pairs (first-seen order); `rows` is one per test, each with a `cells` array aligned 1:1 with `columns` — a `null` cell means that test never ran on that column. Each cell collapses that test × column's repeats into status counts, `score_mean`, `pass_fraction`, `distinct_outputs` (a flakiness signal), `latency_ms_mean`, `cost_usd`, and the cell's `case_keys`. Only `rows` paginate: `limit` (default `100`, max `500`) and `cursor`; columns are always complete.

## Search

| Method | Path                | Scope  | Notes |
|--------|---------------------|--------|-------|
| GET    | `/api/v1/search`    | `read` | SQLite FTS5 across run metadata and case content — names, notes, tags, outputs, and assertion reasons. `q` (required, FTS5 syntax) and `limit`. Returns hits grouped by kind. Matched terms in snippets are wrapped in the private-use characters `U+E000`/`U+E001` for the web UI to split on. |

## Projects, suites, baselines

| Method | Path                                                     | Scope   | Notes |
|--------|----------------------------------------------------------|---------|-------|
| GET    | `/api/v1/projects`                                       | `read`  | List known projects. |
| GET    | `/api/v1/projects/{project}/suites`                      | `read`  | List a project's suites. |
| PUT    | `/api/v1/projects/{project}/suites/{suite}/baseline`     | `write` | Pin a baseline run: body `{ "run_id": "..." }`. `404` if the run is unknown. |
| DELETE | `/api/v1/projects/{project}/suites/{suite}/baseline`     | `write` | Unpin the baseline. `204` on success. |
| GET    | `/api/v1/projects/{project}/suites/{suite}/cases/{case_key}/history` | `read` | One case's status/score/output-hash across the suite's recent runs. `limit` (default `20`, max `100`). |

## Cache (shared provider cache)

The content-addressed cache lets many CI runs share every request domarinn makes — provider responses, grader verdicts, embeddings. Keys are `sha256:<64 hex>`; anything else is a `400`. See [`../concepts/caching.md`](../concepts/caching.md) for the client side.

| Method | Path                       | Scope   | Notes |
|--------|----------------------------|---------|-------|
| GET    | `/api/v1/cache/{key}`      | `read`  | Fetch an entry (`application/octet-stream`). `404` on miss. The only method that moves the hit/miss counters. |
| HEAD   | `/api/v1/cache/{key}`      | `read`  | Existence probe: `200` hit / `404` miss. Deliberately **excluded from the hit/miss counters** — the domarinn client only ever `GET`s, so counting probes would inflate the lookup hit rate the server reports. A found entry still refreshes its last-access time so a probed entry is not evicted next. |
| PUT    | `/api/v1/cache/{key}`      | `write` | Store an entry (first-write-wins: `201` created / `200` already present). `413` if larger than `max_entry_bytes`. |
| GET    | `/api/v1/cache/stats`      | `read`  | `{ entries, total_bytes, hits, misses, oldest_entry_at }`. `hits`/`misses` are `GET` lookups, which is what the web UI's **Lookup hit rate** tile is computed from. |
| POST   | `/api/v1/cache/prune`      | `admin` | Prune by `older_than_days` and/or `target_bytes` (LRU eviction). Returns `{ "pruned": N }`. |

> The server also runs an **hourly retention** task that prunes the cache to
> `DOMARINN_CACHE_MAX_AGE_DAYS` and `DOMARINN_CACHE_MAX_BYTES` automatically;
> `POST /cache/prune` is the manual equivalent.

---

## curl cookbook

Assume `BASE=http://localhost:8321`.

```sh
# Server metadata / is setup required?
curl -s "$BASE/api/v1/meta"

# 1) Create the first admin (only works while zero users exist).
curl -sX POST "$BASE/api/v1/auth/setup" \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"correct horse battery staple"}'

# 2) Log in for a session token (mses_...).
SESSION=$(curl -sX POST "$BASE/api/v1/auth/login" \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"correct horse battery staple"}' \
  | jq -r .token)

# 3) Mint a CI API key at write scope (shown once — capture it now).
curl -sX POST "$BASE/api/v1/apikeys" \
  -H "authorization: Bearer $SESSION" \
  -H 'content-type: application/json' \
  -d '{"name":"ci","scope":"write"}'
# -> { "id": "...", "prefix": "domarinn_xxxxxx", "scope": "write", "key": "domarinn_<64 hex>", ... }

# 4) Upload a run with a token (a static token or an domarinn_ API key).
curl -sX POST "$BASE/api/v1/runs" \
  -H "authorization: Bearer domarinn_ci" \
  -H 'content-type: application/json' \
  --data-binary @result.json
# -> 201 { "id": "<run_id>", "url": "http://localhost:8321/runs/<run_id>" }

# 5) List recent runs for a project.
curl -s "$BASE/api/v1/runs?project=demo&limit=20"
# -> { "runs": [ ... ], "next_cursor": "<cursor|null>" }

# 6) Compare two runs.
curl -s "$BASE/api/v1/runs/$HEAD/compare/$BASE_RUN"
```

In `closed` mode (the default) every call needs at least a `read` token. In `protect-writes` reads work anonymously but writes need a `write` token; in `open` mode you can drop the `Authorization` header entirely.
