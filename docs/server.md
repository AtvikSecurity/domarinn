# The measurellm server & accounts

`measurellm server` runs the results server: a JSON API under `/api/v1` **and**
the embedded React web UI, served from the **same binary**. There is no separate
frontend to deploy, no sidecar database, and no runtime dependency — the binary
is the eval engine, the CLI, the server, and its own container healthcheck.

```sh
measurellm server [--port 8321] [--data-dir /data]
```

| Flag         | Default | Effect |
|--------------|---------|--------|
| `--port`     | `8321`  | Listen port. The server always binds `0.0.0.0`. |
| `--data-dir` | `/data` | State directory (also `MEASURELLM_DATA_DIR`). Holds the SQLite databases. |

The server runs until Ctrl-C (graceful shutdown). Health is exposed at both
`/health` and `/api/v1/health`. See [`./cli.md`](./cli.md) for the rest of the
binary's subcommands and [`./deploy.md`](./deploy.md) for Docker/Kubernetes
hosting.

- [Quick start](#quick-start)
- [Accounts & auth model](#accounts--auth-model)
- [First run: creating the admin](#first-run-creating-the-admin)
- [The API surface](#the-api-surface)
- [Environment variables](#environment-variables)
- [Storage](#storage)
- [Web UI tour](#web-ui-tour)
- [curl cookbook](#curl-cookbook)

---

## Quick start

```sh
# Open mode — anyone can read and write. Good for a laptop or a trusted network.
measurellm server --data-dir ./data
# UI + API on http://localhost:8321
```

The moment you add credentials the server locks writes down automatically — see
[the auth model](#accounts--auth-model) below.

```sh
# Bootstrap an admin and require a token for writes, in one shot.
MEASURELLM_ADMIN_USER=admin \
MEASURELLM_ADMIN_PASSWORD='correct horse battery staple' \
MEASURELLM_TOKENS='write:mllm_ci,admin:mllm_ops' \
measurellm server --data-dir ./data
```

---

## Accounts & auth model

measurellm has **three auth modes**. You rarely set one explicitly: the
effective mode is *derived* from whether any credentials exist.

| Mode             | Reads / UI | Writes (ingest, baseline, cache PUT) | Admin (delete, prune, users) |
|------------------|------------|--------------------------------------|------------------------------|
| `open`           | open       | open                                 | open                         |
| `protect-writes` | open       | require `write`                      | require `admin`              |
| `closed`         | require `read` | require `write`                  | require `admin`              |

**How the mode is chosen (at startup):**

1. If `MEASURELLM_AUTH_MODE` is set (`open` \| `protect-writes` \| `closed`), it
   wins outright. (`protect_writes` with an underscore is also accepted.)
2. Otherwise, if **any** static token *or* **any** local user account exists, the
   mode defaults to **`protect-writes`**.
3. Otherwise it is **`open`**.

So a fresh, credential-less instance is fully open; the first token you configure
or the first account you create flips it to `protect-writes`. The active mode is
reported by `GET /api/v1/meta` as `auth_mode`.

> Scopes are ordered: **`admin` ⊃ `write` ⊃ `read`**. A route asks for a minimum
> scope; a higher scope always satisfies it. In `open` mode no scope is required
> at all; in `protect-writes` the `read` requirement is waived but `write`/`admin`
> are enforced; in `closed` every requirement is enforced.

### Two kinds of credentials

Both are presented the same way — as a bearer token in the `Authorization`
header (see [The auth header](#the-auth-header)).

**1. Static bearer tokens** — configured via the environment, no database rows,
no user. Ideal for CI and bootstrapping.

```
MEASURELLM_TOKENS="read:mllm_view,write:mllm_ci,admin:mllm_ops"
```

Each comma-separated entry is `scope:secret`, where scope is `read`, `write`, or
`admin`. The secret string is whatever you choose (the `mllm_` names above are
illustrative). Static tokens are matched in constant time and are **not** tied to
a user account — so they cannot create API keys or appear in the users list.

**2. Local user accounts** — real username/password logins stored in SQLite.

- Passwords are **argon2**-hashed (minimum 8 characters).
- Two roles: **`admin`** and **`member`**. Role maps to a scope ceiling:
  `admin → admin`, `member → write`.
- Logging in mints a **session** (token prefix `mses_`, 30-day lifetime). The
  browser UI uses sessions; `POST /auth/logout` revokes the presenting one.
- Each account can mint **API keys** (prefix `mllm_`, 256 bits of entropy). The
  secret is shown **exactly once** on creation, is revocable, and carries a
  **scope ceiling** — a key may be created at or below the creator's own scope,
  never above it.

| Credential   | Prefix   | Backed by | Can manage accounts/keys? | Typical use |
|--------------|----------|-----------|---------------------------|-------------|
| Static token | (any)    | env var   | no                        | CI, bootstrap |
| Session      | `mses_`  | account   | yes (as the user)         | Web UI login |
| API key      | `mllm_`  | account   | yes (as the user)         | Scripts, CI tied to a user |

The authenticator chain resolves a presented token in order — **static token →
API key → session** — dispatching account lookups by prefix so at most one DB
hit occurs per request.

### The auth header

```
Authorization: Bearer <static-token | mllm_apikey | mses_session>
```

The `Bearer ` prefix is recommended; a bare token value in the header is also
accepted. The same header works for every credential kind — the server figures
out which one you presented.

---

## First run: creating the admin

Until an admin account exists, `GET /api/v1/meta` reports `"setup_required":
true`. There are **two** ways to create that first admin:

**A. Interactive setup (via the UI or the API).** `POST /api/v1/auth/setup` with
`{username, password}` creates the first admin and returns a session token. This
endpoint is open **only while zero users exist**; afterwards it is a `409`. The
web UI's `/setup` page drives exactly this call.

```sh
curl -sX POST http://localhost:8321/api/v1/auth/setup \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"correct horse battery staple"}'
# 201 { "token": "mses_...", "user": { "id": "...", "username": "admin", "role": "admin", ... } }
```

**B. Bootstrap from the environment.** Set `MEASURELLM_ADMIN_USER` and
`MEASURELLM_ADMIN_PASSWORD`. On every startup the server **idempotently** ensures
that account exists as an enabled admin, creating it if missing and updating the
password if it changed. This is the right choice for containers and Kubernetes —
declare the admin in your secret store and the instance self-seeds.

> Because the bootstrap admin is created *before* the mode is derived, an
> instance seeded this way comes up in `protect-writes` (not `open`).

---

## The API surface

All endpoints live under `/api/v1` (health is also mirrored at the bare
`/health`). Responses are JSON; errors render as `{ "error": "<message>" }` with
an appropriate status. The **Scope** column is the route's *minimum* scope — what
it demands in `closed` mode, and (for `write`/`admin`) in `protect-writes`. In
`open` mode nothing is required.

Request-body size is capped at **64 MiB**; request bodies may be gzip/deflate
compressed (the server decompresses transparently).

### Health & meta

| Method | Path              | Scope | Notes |
|--------|-------------------|-------|-------|
| GET    | `/health`         | —     | `{ "status": "ok" }`. Also the container healthcheck target. |
| GET    | `/api/v1/health`  | —     | Same as above. |
| GET    | `/api/v1/meta`    | —     | Server metadata (below). Always open. |

`GET /api/v1/meta` returns:

```json
{
  "name": "measurellm",
  "version": "0.1.0",
  "auth_mode": "protect-writes",
  "setup_required": false,
  "supported_schema_versions": [1, 2],
  "result_schema_version": 2,
  "cache": { "max_entry_bytes": 4194304, "max_bytes": 1073741824, "max_age_days": 30 }
}
```

### Auth (accounts, sessions)

| Method | Path                    | Scope | Notes |
|--------|-------------------------|-------|-------|
| POST   | `/api/v1/auth/setup`    | —     | Create the first admin. Open only while zero users exist, else `409`. Returns a session token. |
| POST   | `/api/v1/auth/login`    | —     | Exchange `{username, password}` for a session token. `401` on bad/disabled credentials. |
| POST   | `/api/v1/auth/logout`   | (authenticated) | Revoke the presenting session. No-op `200` for token/API-key callers; `401` for anonymous. |
| GET    | `/api/v1/auth/me`       | —     | Report the current identity: `{authenticated, user, source, scope}`. `source` is `anonymous` \| `static` \| `apikey` \| `session`. |

### API keys

These require an **account-backed** identity (session or API key). A static
token has no owning user and gets a `403` here.

| Method | Path                    | Scope   | Notes |
|--------|-------------------------|---------|-------|
| GET    | `/api/v1/apikeys`       | `write` | List the caller's own keys (never the secret). |
| POST   | `/api/v1/apikeys`       | `write` | Mint a key: `{name?, scope?}`. Scope defaults to the caller's own and may not exceed it (`403`). Returns the secret **once** as `key`. |
| DELETE | `/api/v1/apikeys/{id}`  | `write` | Revoke a key. Allowed for its owner or any admin, else `403`. |

### Users administration

| Method | Path                    | Scope   | Notes |
|--------|-------------------------|---------|-------|
| GET    | `/api/v1/users`         | `admin` | List all accounts. |
| POST   | `/api/v1/users`         | `admin` | Create an account: `{username, password, role}` (`role` = `admin`\|`member`). |
| PATCH  | `/api/v1/users/{id}`    | `admin` | Update `role`, `disabled`, and/or `password` (any subset). |
| DELETE | `/api/v1/users/{id}`    | `admin` | Delete an account. Refuses the **last admin** (`409`). |

### Runs

| Method | Path                                    | Scope   | Notes |
|--------|-----------------------------------------|---------|-------|
| POST   | `/api/v1/runs`                          | `write` | Ingest a run document. See [ingest](#run-ingest). |
| GET    | `/api/v1/runs`                          | `read`  | List runs (filterable, paginated). |
| GET    | `/api/v1/runs/{id}`                      | `read`  | Full run detail. `404` if unknown. |
| GET    | `/api/v1/runs/{id}/cases`               | `read`  | Lean list of the run's cases (filterable, paginated). |
| GET    | `/api/v1/runs/{id}/cases/{case_key}`    | `read`  | One case's full detail. |
| GET    | `/api/v1/runs/{id}/export`              | `read`  | The original, lossless run document. |
| GET    | `/api/v1/runs/{id}/compare/{other}`     | `read`  | Diff two runs (regressions/improvements per case). |
| DELETE | `/api/v1/runs/{id}`                      | `admin` | Delete a run. `204` on success. |

<a id="run-ingest"></a>**Ingest** (`POST /api/v1/runs`) accepts a `RunResult`
JSON document (see [`./protocol.md`](./protocol.md) and `measurellm schema
result`). The body must carry a `schema_version` within the supported window
(`result_schema_version - 1 ..= result_schema_version`), else `422`. Ingest is
**idempotent by content**:

| Status | Meaning |
|--------|---------|
| `201 Created` | New run stored. Body: `{ "id", "url" }`. |
| `200 OK`      | Identical run id + content already existed. Body: `{ "id", "url" }`. |
| `409 Conflict`| Same run id, **different** content. |

The `url` in the response is a browser link to the run. It is built from
`MEASURELLM_PUBLIC_URL` when set; otherwise from the request's `Host` header and
`X-Forwarded-Proto` (see [`./deploy.md`](./deploy.md#reverse-proxies)).

**List filters** (`GET /api/v1/runs`, all optional query params): `project`,
`suite`, `tag`, `branch`, `status`, `since`, `until` (each epoch-ms *or*
RFC3339), `limit` (default `50`, max `200`), `cursor`. The response is
`{ "runs": [...], "next_cursor": "<cursor|null>" }`; pass `next_cursor` back as
`cursor` to page.

**Case filters** (`GET /api/v1/runs/{id}/cases`): `status`, `tag`, `q`
(free-text), `limit`, `cursor`.

### Projects, suites, baselines

| Method | Path                                                     | Scope   | Notes |
|--------|----------------------------------------------------------|---------|-------|
| GET    | `/api/v1/projects`                                       | `read`  | List known projects. |
| GET    | `/api/v1/projects/{project}/suites`                      | `read`  | List a project's suites. |
| PUT    | `/api/v1/projects/{project}/suites/{suite}/baseline`     | `write` | Pin a baseline run: body `{ "run_id": "..." }`. `404` if the run is unknown. |
| DELETE | `/api/v1/projects/{project}/suites/{suite}/baseline`     | `write` | Unpin the baseline. `204` on success. |

### Cache (shared provider cache)

The content-addressed cache lets many CI runs share provider outputs. Keys are
`sha256:<64 hex>`; anything else is a `400`. See [`./caching.md`](./caching.md)
for the client side.

| Method | Path                       | Scope   | Notes |
|--------|----------------------------|---------|-------|
| GET    | `/api/v1/cache/{key}`      | `read`  | Fetch an entry (`application/octet-stream`). `404` on miss. |
| HEAD   | `/api/v1/cache/{key}`      | `read`  | Existence probe: `200` hit / `404` miss. |
| PUT    | `/api/v1/cache/{key}`      | `write` | Store an entry (first-write-wins: `201` created / `200` already present). `413` if larger than `max_entry_bytes`. |
| GET    | `/api/v1/cache/stats`      | `read`  | `{ entries, total_bytes, hits, misses, oldest_entry_at }`. |
| POST   | `/api/v1/cache/prune`      | `admin` | Prune by `older_than_days` and/or `target_bytes` (LRU eviction). Returns `{ "pruned": N }`. |

> The server also runs an **hourly retention** task that prunes the cache to
> `MEASURELLM_CACHE_MAX_AGE_DAYS` and `MEASURELLM_CACHE_MAX_BYTES` automatically;
> `POST /cache/prune` is the manual equivalent.

---

## Environment variables

**Read by the server** (`measurellm server`):

| Variable                        | Default        | Purpose |
|---------------------------------|----------------|---------|
| `MEASURELLM_DATA_DIR`           | `/data`        | State directory. Holds `measurellm.db` and `cache.db`. Also settable with `--data-dir`. |
| `MEASURELLM_TOKENS`             | (unset)        | Static bearer tokens as `scope:secret` pairs, comma-separated. Configuring any flips the default mode to `protect-writes`. |
| `MEASURELLM_AUTH_MODE`          | (derived)      | Force the mode: `open` \| `protect-writes` \| `closed`. Overrides the derivation. |
| `MEASURELLM_ADMIN_USER`         | (unset)        | Bootstrap admin username. Requires the password too. |
| `MEASURELLM_ADMIN_PASSWORD`     | (unset)        | Bootstrap admin password. The account is (re)ensured on every startup. |
| `MEASURELLM_PUBLIC_URL`         | (unset)        | Public base URL for share links / absolute URLs. No trailing slash, no path prefix. |
| `MEASURELLM_CACHE_MAX_ENTRY_BYTES` | `4194304` (4 MiB)   | Max size of a single cache entry. |
| `MEASURELLM_CACHE_MAX_BYTES`    | `1073741824` (1 GiB) | Total cache size target for retention. |
| `MEASURELLM_CACHE_MAX_AGE_DAYS` | `30`           | Cache entry max age for retention. |
| `RUST_LOG`                      | (off)          | Log filter, e.g. `RUST_LOG=measurellm=debug`. Logs go to stderr. |

**Read by the CLI** (when uploading runs / using an HTTP cache — *not* the
server):

| Variable                | Purpose |
|-------------------------|---------|
| `MEASURELLM_SERVER_URL` | Target server base URL for `measurellm run --share` / `share` (or the `--server-url` flag). |
| `MEASURELLM_TOKEN`      | A single bearer token the CLI sends when uploading a run or using the HTTP cache backend. |

See [`./cli.md`](./cli.md) and [`./caching.md`](./caching.md) for the client
side.

---

## Storage

State lives in the data directory as **two SQLite files** (WAL mode):

| File            | Contents | Back up? |
|-----------------|----------|----------|
| `measurellm.db` | Durable run history. Each run is stored both as a compressed lossless blob (for export) and as normalized rows for indexed filtering. Also holds users, sessions, API keys, and baselines. | **Yes — this is the backup target.** |
| `cache.db`      | The content-addressed provider cache. Regenerable. | No — disposable. |

SQLite is a **single writer**: run exactly one instance against a given data
directory. This is what makes backups a file copy and self-hosting a one-liner —
and why the deployment guidance is single-replica. Migrations run automatically
at startup. See [`./deploy.md`](./deploy.md) for backup and Kubernetes details.

---

## Web UI tour

The UI is a single-page app served at the web root (client-side routing; give the
service its own hostname, not a path prefix).

| Path                          | Page | What it does |
|-------------------------------|------|--------------|
| `/`                           | Runs list | Browse and filter runs (project, suite, tag, branch, status). |
| `/runs/:id`                   | Run detail | The **cases × asserts grid**; click a cell to open the **detail drawer** with the case's output and assertion evidence. |
| `/runs/:id/compare[/:other]`  | Compare | Base/head run pickers with **regression highlighting** (newly failing cases stand out). |
| `/cache`                      | Cache stats | Entry count, total size, hit/miss counters. |
| `/settings`                   | Settings | Instance/account settings. |
| `/login`                      | Login | Username/password sign-in (mints a session). |
| `/setup`                      | Setup | First-run admin creation (only while `setup_required`). |
| `/keys`                       | API keys | Create, view (once), and revoke your API keys. |
| `/admin`                      | Admin users | Manage accounts (admin-only; guarded in the UI). |

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
# -> { "id": "...", "prefix": "mllm_xxxxxx", "scope": "write", "key": "mllm_<64 hex>", ... }

# 4) Upload a run with a token (a static token or an mllm_ API key).
curl -sX POST "$BASE/api/v1/runs" \
  -H "authorization: Bearer mllm_ci" \
  -H 'content-type: application/json' \
  --data-binary @result.json
# -> 201 { "id": "<run_id>", "url": "http://localhost:8321/runs/<run_id>" }

# 5) List recent runs for a project.
curl -s "$BASE/api/v1/runs?project=demo&limit=20"
# -> { "runs": [ ... ], "next_cursor": "<cursor|null>" }

# 6) Compare two runs.
curl -s "$BASE/api/v1/runs/$HEAD/compare/$BASE_RUN"
```

In `open` mode you can drop the `Authorization` header entirely. In
`protect-writes` (the default once credentials exist) reads work anonymously but
writes need a `write` token; in `closed` every call needs at least a `read`
token.
