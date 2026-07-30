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

## SSO (only present when configured — see [Single sign-on](server.md#single-sign-on-oidc--saml))

| Method | Path                    | Scope | Notes |
|--------|-------------------------|-------|-------|
| GET    | `/api/v1/auth/oidc/{provider}/start` | — | Begin an OIDC login; `303` to the IdP. `?return_to=/path` deep-links back. |
| GET    | `/api/v1/auth/oidc/{provider}/callback` | — | OIDC redirect target; `303` home or to `/login?sso_error=…`. |
| GET    | `/api/v1/auth/saml/{provider}/start` | — | Begin a SAML login; `303` to the IdP (redirect binding). |
| POST   | `/api/v1/auth/saml/{provider}/acs` | — | SAML assertion consumer (HTTP-POST binding). |
| GET    | `/api/v1/auth/saml/{provider}/metadata` | — | SP metadata XML for the IdP to import. |

## API keys

These require an **account-backed** identity (session or API key). A static token has no owning user and gets a `403` here.

The scope gate is `read` rather than `write` so a `viewer` account can mint the read-only key its role exists for. Nothing is loosened by that: the account check above still applies, and a minted key can never exceed the caller's own scope.

| Method | Path                    | Scope   | Notes |
|--------|-------------------------|---------|-------|
| GET    | `/api/v1/apikeys`       | `read`  | List the caller's own keys (never the secret). |
| POST   | `/api/v1/apikeys`       | `read`  | Mint a key: `{name?, scope?}`. Scope defaults to the caller's own and may not exceed it (`403`). Returns the secret **once** as `key`. |
| DELETE | `/api/v1/apikeys/{id}`  | `read`  | Revoke a key. Allowed for its owner or any admin, else `403`. |

## Users administration

| Method | Path                    | Scope   | Notes |
|--------|-------------------------|---------|-------|
| GET    | `/api/v1/users`         | `admin` | List all accounts. |
| POST   | `/api/v1/users`         | `admin` | Create an account: `{username, password, role}` (`role` = `admin`\|`member`\|`viewer`). |
| PATCH  | `/api/v1/users/{id}`    | `admin` | Update `role`, `disabled`, and/or `password` (any subset). |
| DELETE | `/api/v1/users/{id}`    | `admin` | Delete an account. Refuses the **last admin** (`409`). |

## Runs

| Method | Path                                    | Scope   | Notes |
|--------|-----------------------------------------|---------|-------|
| POST   | `/api/v1/runs`                          | `write` | Ingest a run document. See [ingest](#run-ingest). Uploading into a **restricted** run set additionally needs an `upload` grant (`403`); see [run sets](#run-sets-access-control). |
| GET    | `/api/v1/runs`                          | `read`  | List runs (filterable, paginated). Filters: `project`, `suite`, `tag`, `branch`, `since`, `until`, `status`, `cached`, `origin` (`ci`\|`local`), `actor`. |
| GET    | `/api/v1/runs/{id}`                      | `read`  | Full run detail. `404` if unknown. |
| GET    | `/api/v1/runs/{id}/cases`               | `read`  | Lean list of the run's cases (filterable, paginated). |
| GET    | `/api/v1/runs/{id}/cases/{case_key}`    | `read`  | One case's full detail. |
| GET    | `/api/v1/runs/{id}/matrix`              | `read`  | Prompt × provider aggregate matrix (rows = tests, paginated). |
| GET    | `/api/v1/runs/{id}/export`              | `read`  | The original, lossless run document. |
| GET    | `/api/v1/runs/{id}/config`              | `read`  | The run's config digest + snapshot (no full export). |
| GET    | `/api/v1/runs/{id}/compare/{other}`     | `read`  | Diff two runs (regressions/improvements per case). |
| DELETE | `/api/v1/runs/{id}`                      | `admin` | Delete a run. `204` on success. |

**Every read above is filtered by what the caller may see.** A run in a restricted set is omitted from the list (and from the `cached_hidden` count beside it), and addressing it by id is a `404` rather than a `403` — the same answer a run that never existed gets. See [run sets](#run-sets-access-control).

`GET /runs/{id}` reports two cost figures, and they are never summed: `cost_usd` is what the systems under test cost, `grader_cost_usd` is what grading them cost. It also carries `cache_read_tokens`, `cache_write_tokens` and `cache_savings_usd`. All four are `null` for runs ingested before the columns existed — which is **not** the same as zero, and readers render it as unknown rather than as "no activity". There is no backfill; see the migration note in `storage/schema.rs` for why.

<a id="run-ingest"></a>**Ingest** (`POST /api/v1/runs`) accepts a `RunResult`
JSON document (see [`protocol.md`](protocol.md) and `domarinn schema result`). The body must carry a `schema_version` within the supported window (`result_schema_version - 1 ..= result_schema_version`), else `422`. Ingest is **idempotent by content**:

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
| GET    | `/api/v1/search`    | `read` | SQLite FTS5 across run metadata and case content — names, notes, tags, outputs, and assertion reasons. `q` (required, FTS5 syntax) and `limit`. Returns hits grouped by kind, filtered to the runs the caller may see. Matched terms in snippets are wrapped in the private-use characters `U+E000`/`U+E001` for the web UI to split on. |

## Projects, suites, baselines

| Method | Path                                                     | Scope   | Notes |
|--------|----------------------------------------------------------|---------|-------|
| GET    | `/api/v1/projects`                                       | `read`  | List known projects, restricted ones omitted. |
| GET    | `/api/v1/projects/{project}/suites`                      | `read`  | List a project's suites, restricted ones omitted. |
| PUT    | `/api/v1/projects/{project}/suites/{suite}/baseline`     | `write` | Pin a baseline run: body `{ "run_id": "..." }`. `200` with `{project, suite, run_id}` on success. Needs an `upload` grant when the suite is restricted (`403`). `404` if the run is unknown, invisible to the caller, or belongs to another suite. |
| DELETE | `/api/v1/projects/{project}/suites/{suite}/baseline`     | `write` | Unpin the baseline. Same `upload` gate. `204` on success. |
| GET    | `/api/v1/projects/{project}/suites/{suite}/cases/{case_key}/history` | `read` | One case's status/score/output-hash across the suite's recent runs. `limit` (default `20`, max `100`). |

A baseline is a claim about a suite, so it may only name a run **the caller can see, in that same suite**. All three failures — no such run, someone else's suite, restricted away — are one `404`, for the reason the [run-set section](#run-sets-access-control) gives.

<a id="run-sets-access-control"></a>

## Run sets & access control

A **run set** is a `(project, suite)` pair — the grouping every run already declares. `/api/v1/sets*` browses those sets and edits who may reach them.

The model is **default-open**: until someone restricts a set, it behaves exactly as it did before this surface existed. Restricting one hides its runs from everyone but the people granted it.

| Method | Path | Scope | Notes |
|--------|------|-------|-------|
| GET | `/api/v1/sets` | `read` | Every project the caller may see, with per-project totals, one latest pass rate per suite, and the caller's own grant. Never paginated. |
| GET | `/api/v1/sets/{project}` | `read` | One project's suites, each with totals, a pass-rate sparkline and its pinned baseline. `404` if it does not exist **or** is restricted away. |
| GET | `/api/v1/sets/{project}/suites/{suite}` | `read` | The same aggregates for one suite, addressed directly. `404` on the same terms. |
| GET | `/api/v1/sets/{project}/access` | `read` | The project's access list: `{ restricted, grants: [...] }`. Admin, or a covering `manage` grant — else `404`. |
| GET | `/api/v1/sets/{project}/suites/{suite}/access` | `read` | The suite's own access list, on the same gate. |
| PUT | `/api/v1/sets/{project}/restriction` | `admin` | Restrict the whole project. Idempotent; `204`. |
| DELETE | `/api/v1/sets/{project}/restriction` | `admin` | Lift it, keeping the grants. `204`, or `404` if there was no such row. |
| PUT | `/api/v1/sets/{project}/suites/{suite}/restriction` | `admin` | Restrict one suite. `204`. |
| DELETE | `/api/v1/sets/{project}/suites/{suite}/restriction` | `admin` | Lift it. `204` / `404`. |
| PUT | `/api/v1/sets/{project}/grants/{user_id}` | `write` | Grant or re-level a user on the project: body `{ "level": "view" \| "upload" \| "manage" }`. `204`. `404` for an unknown user. |
| DELETE | `/api/v1/sets/{project}/grants/{user_id}` | `write` | Revoke it. `204`, or `404` if there was no such grant. |
| PUT | `/api/v1/sets/{project}/suites/{suite}/grants/{user_id}` | `write` | The suite-scoped grant, same body. |
| DELETE | `/api/v1/sets/{project}/suites/{suite}/grants/{user_id}` | `write` | Revoke it. |

The literal `access` / `restriction` / `grants` segments sit where a suite name could otherwise go, which is why the suite forms spell `/suites/` out: `/sets/{project}/suites/{suite}` can never collide with `/sets/{project}/access`, whatever anyone calls a project or a suite.

`GET /api/v1/sets` returns:

```json
{
  "projects": [
    {
      "project": "checkout",
      "suite_count": 2,
      "run_count": 4,
      "last_run_at": 1767225600000,
      "pass_count": 4,
      "fail_count": 3,
      "error_count": 0,
      "case_count": 7,
      "recent_pass_rates": [0.5, 1.0],
      "restricted": true,
      "my_level": "manage"
    }
  ]
}
```

Four fields on that body are easy to misread:

- `last_run_at` — and every other timestamp on this surface — is integer **epoch-ms**, not the RFC3339 strings `/api/v1/projects` emits.
- `recent_pass_rates` is one latest rate per suite, suites in name order and capped: the spread across the project, not a time series. The per-suite `sparkline` on the detail endpoints is the time series.
- `my_level` is `null` both for callers that do not ride grants at all — admins, anonymous callers, static tokens — and for a user who simply holds none, which are indistinguishable on the wire. On a `/sets` row (and on `ProjectSetDetail` itself) it reports only **project-scoped** grants, so a user whose only grant is on one suite reads `null` here and sees their level on that suite's row instead.
- `restricted` does not mean the same thing everywhere. See below.

### Two meanings of `restricted`

The browse views answer "is this set locked for me right now", and the access endpoint answers "does this set carry a restriction row of its own". Those differ exactly when a project-level row covers a suite:

| Field | Scope | A suite inside a restricted project |
|---|---|---|
| `ProjectSetView.restricted` (`GET /sets`) and `ProjectSetDetailResponse.restricted` | The project's own row — the only thing that can cover a project | n/a |
| `SuiteSetView.restricted` and `SuiteSetDetailResponse.restricted` | **Covering**: its own row *or* the project's | `true` |
| `SetAccessResponse.restricted` (`GET .../access`) | **Exact**: the row this scope's `PUT`/`DELETE .../restriction` writes and removes | `false` |

Two consequences worth stating outright, because both look like bugs:

- **A project whose suites are individually locked reports `restricted: false`.** The project row is what that field is about; the locked suites say so on their own rows. Only a project-level restriction makes the project itself read `true`.
- **A suite's access endpoint reports `restricted: false` while the suite is unreachable.** Its restriction lives on the project, and the project's access endpoint is where it is lifted — which is precisely what that field is for: it drives the toggle beside it, and a toggle here would delete a row that does not exist. Ask the browse endpoints, not the access endpoint, whether a set is hidden.

`GET .../access` is likewise **exact-scope in its `grants`**: it lists the grants recorded at that scope, not the project-level grants that also cover the suite. A caller's *effective* level always covers; an access list never does.

### Restrictions and grants

- A **restriction** row covers either a whole project (`PUT /sets/{project}/restriction`) or a single suite. A project-level row covers every suite in it, including suites uploaded afterwards.
- A **grant** names one user on one set at a level: `view` < `upload` < `manage`, each including the ones below it. A project-level grant covers every suite in the project.
- Restricting a set is a fact about the `(project, suite)` pair, not about its runs — you may restrict and grant a set **before** its first run is ever uploaded.
- Lifting a restriction keeps the grants, so re-restricting the set restores the access list it had.
- A run with **no project** can never be restricted: no restriction row can name it, so it is always visible.

`manage` is deliberately outside the default-open waiver. On an unrestricted set `view` and `upload` are free to everyone — that is what default-open means — but `manage` is the power to hand out grants on a set, including to yourself, so it always takes admin or an explicit covering `manage` grant. Without that carve-out, any caller who could reach a write endpoint could seize a project nobody had restricted yet, which on a fresh instance is every project.

A `manage` grant is honoured whether or not the set is restricted, and that is not a dormant technicality: on an open set its holder can already read and rewrite that set's access list today, and every grant they add is live the moment an admin locks the set. Treat handing out `manage` on an unrestricted set as the delegation it is.

### Two independent gates

Every endpoint above passes through a **route scope** (the deployment's auth mode, the `Scope` column) and, where it touches policy, a **set gate** (the caller's grant over that set). They answer different questions — "is this credential strong enough" and "does this principal own this set" — and neither substitutes for the other, because a grant belongs to a *user* and every user-backed credential rides its owner's grants. Without the scope gate, the least privileged credential the product offers — a `read` API key — would inherit its owner's `manage` grant and be able to rewrite an access list.

That is also why the refusals differ:

- **The browse reads and the access lists refuse with `404`.** An access list names the people who can reach a restricted set, so "you may not see this" and "there is nothing here" have to be the same answer. Note that the gate on an access list is `manage`, which the default-open waiver never hands out: reading the access list of a set you do not manage is a `404` even when the set itself is wide open and you can browse it freely.
- **The scope gate refuses with `403`.** By the time it can matter the caller has already proved they hold the grant, so the only thing left to say is that their credential is too weak. Reading an access list is `read`-scoped (a `viewer` who manages a set may look at it); `write` is what stops them — or any read-scoped key — from changing it.
- **`POST /runs` and the baseline routes refuse with `403` too**, and say which level was missing. Unlike a run id, the `(project, suite)` there comes from the caller, so refusing it discloses nothing they did not already name — and a silent `404` on a legitimate upload would be a debugging trap for whoever runs the CI job.

### Which credentials see what

| Caller | Sees | Notes |
|--------|------|-------|
| `admin` role, via any credential | everything | Also manages every set. |
| Static token at `admin` scope | everything | An operator credential. |
| Session or API key (`member` or `viewer`) | unrestricted sets + whatever their user is granted | An API key rides its **owner's** grants, whatever scope the key was minted at. |
| Anonymous, and static tokens at `read`/`write` | unrestricted sets only | |

**A `read` or `write` static token deliberately does not pierce restrictions.** Those are shared, widely-copied secrets with no owning user and therefore no grants; if a `write` token could reach restricted sets, every CI job in the deployment could. To let CI upload into a restricted set, create a bot **user account**, grant it `upload` on the set, and give the job that account's API key.

**An `admin:` static token is not contained by any of this**, as the table above says: admin scope resolves to full visibility, so such a token reads and manages every restricted set on the instance. That is intended — it is the operator credential — but it means "we gave CI a static token" is only a containment argument for the `read` and `write` ones.

Restrictions apply to every surface that reads runs, not just the ones above: the runs list and run detail, cases, matrix, export, compare, search, case history, the projects and suites listings, the run links on a cache entry, and the [MCP tools](mcp.md). There is one derivation of a caller's access class in the server, and every one of those queries appends the same predicate to its SQL.

## Cache (shared provider cache)

The content-addressed cache lets many CI runs share every request domarinn makes — provider responses, grader verdicts, embeddings. Keys are `sha256:<64 hex>`; anything else is a `400`. See [`../concepts/caching.md`](../concepts/caching.md) for the client side.

| Method | Path                       | Scope   | Notes |
|--------|----------------------------|---------|-------|
| GET    | `/api/v1/cache/{key}`      | `read`  | Fetch an entry (`application/octet-stream`). `404` on miss. The only method that moves the hit/miss counters. |
| HEAD   | `/api/v1/cache/{key}`      | `read`  | Existence probe: `200` hit / `404` miss. Deliberately **excluded from the hit/miss counters** — the domarinn client only ever `GET`s, so counting probes would inflate the lookup hit rate the server reports. A found entry still refreshes its last-access time so a probed entry is not evicted next. |
| PUT    | `/api/v1/cache/{key}`      | `write` | Store an entry (first-write-wins: `201` created / `200` already present). `413` if larger than `max_entry_bytes`. |
| GET    | `/api/v1/cache/stats`      | `read`  | `{ entries, total_bytes, hits, misses, unindexed, oldest_entry_at }`. `hits`/`misses` are `GET` lookups, which is what the web UI's **Lookup hit rate** tile is computed from. |
| POST   | `/api/v1/cache/prune`      | `admin` | Prune by `older_than_days` and/or `target_bytes` (LRU eviction). Returns `{ "pruned": N }`. |

> The server also runs an **hourly retention** task that prunes the cache to
> `DOMARINN_CACHE_MAX_AGE_DAYS` and `DOMARINN_CACHE_MAX_BYTES` automatically;
> `POST /cache/prune` is the manual equivalent.

### Browsing the cache

| Method | Path | Scope | Notes |
|--------|------|-------|-------|
| GET | `/api/v1/cache/entries` | `admin` | List entries. Filters: `kind`, `model`, `q`, `since`, `until`, `min_cost_microusd`, `max_cost_microusd`, `tier`. Sort: `sort=created\|last_access\|size\|cost` with `order=desc\|asc`. Paginates by opaque `cursor` (`limit` default `50`, max `200`). |
| GET | `/api/v1/cache/entries/{key}` | `read` | One entry, parsed. `?raw=true` also returns the provider's raw metadata, which is withheld by default. |
| GET | `/api/v1/cache/entries/{key}/runs` | `read` | Cases whose provider call was addressed by this key, newest run first. Cursor paginated. |
| GET | `/api/v1/cache/facets` | `admin` | Values the `kind` and `model` filters can take, with counts, plus `total` / `unindexed` / `unparseable`. |

**Listing is `admin`; reading one entry is `read`.** That asymmetry is deliberate.
A key is `sha256(canonical_json({request, repeat, salts}))` — the only way to
compute one is to already possess the exact prompt, model and parameters — so
`GET /cache/{key}` at `read` scope never revealed anything the caller did not
already have. *Enumerating* is a different capability: it turns "you already
know the prompt" into every prompt and every response, sorted by cost and
searchable. Two facts decide where that belongs: `read` is the **anonymous**
scope under `protect-writes`, and `write` is the CI-token scope. So listing sits
with `POST /cache/prune` at `admin` — destroying the corpus and enumerating it
are comparable powers.

Note one genuinely new exposure: in a `layered` setup a developer's local
iteration writes cache entries to the shared server even when the run itself is
never `--share`d. Those prompts were already on the server; listing is what
makes them reachable.

**Browsing never moves the counters.** Neither listing nor inspecting touches
`hits`/`misses` (which would make the lookup hit-rate tile lie) or
`last_access_at` (which is what `POST /cache/prune` evicts on — a browse that
refreshed it would let looking at the cache change what the cache evicts). Both
read through a pooled read-only connection, so this is a property of the
connection rather than of discipline.

Two `kind=` pseudo-values exist because a real kind filter provably cannot match
rows nothing is known about: `kind=unindexed` selects entries the backfill has
not reached, and `kind=unparseable` selects entries whose body this server could
not read. Without them those rows would be unreachable exactly while someone
wants to look at them.

`sort=cost` lists only entries whose cost is known. Ordering by an unknown value
is meaningless, and the NULL tail would also stop keyset pagination dead.

**An empty `…/runs` list is ambiguous, and readers must say so.** A case carries
its cache key only if it was recorded by a version that wrote one, and older
runs cannot be backfilled: the key is derived from the canonical request plus
the repeat index and salts, and a stored run document contains none of those —
a backfill would produce *wrong* keys, not missing ones. So "no runs" means
"nothing on this server records having used it", never "this entry is unused".

### Indexed metadata

An entry's `body` is opaque to the server: it is stored and served byte-for-byte,
and a `PUT` the server cannot parse still succeeds. Alongside that, the server
*tries* to read each entry once, and promotes what it finds — kind, model, cost,
token counts, a request summary and an output preview — into indexed columns so
entries can be listed, filtered and searched without decoding every row.

New entries are indexed on the way in. Entries already in a database when it was
upgraded are indexed by a **background task** that drains in small batches
alongside normal traffic, rather than blocking startup. `stats.unindexed` counts
what it has left; `0` is the steady state.

Two consequences worth knowing:

- Entries that are not yet indexed are still listed, never hidden — but a
  `kind=` or `model=` filter cannot match them, and full-text search cannot
  reach them, because nothing about them has been established yet.
- An entry whose body the server cannot parse is indexed as *unparseable* once
  and never re-examined. It stays fully readable through `GET /cache/{key}`;
  it simply carries no metadata.

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

# 7) Lock a run set and let one account upload into it (admin session).
#    Restricting is admin-only; granting needs write scope + manage on the set.
curl -sX PUT "$BASE/api/v1/sets/checkout/suites/regression/restriction" \
  -H "authorization: Bearer $SESSION"
# -> 204

# $USER_ID is the `id` of the account to grant, from `GET /api/v1/users`
# (admin scope) or from the body `POST /api/v1/users` returned when you
# created the bot account. It is never the username.
curl -sX PUT "$BASE/api/v1/sets/checkout/suites/regression/grants/$USER_ID" \
  -H "authorization: Bearer $SESSION" \
  -H 'content-type: application/json' \
  -d '{"level":"upload"}'
# -> 204

# 8) Read back who may reach it (admin, or a covering manage grant; else 404).
curl -s "$BASE/api/v1/sets/checkout/suites/regression/access" \
  -H "authorization: Bearer $SESSION"
# -> { "restricted": true, "grants": [ { "username": "ci-bot", "level": "upload", ... } ] }
```

In `closed` mode (the default) every call needs at least a `read` token. In `protect-writes` reads work anonymously but writes need a `write` token; in `open` mode you can drop the `Authorization` header entirely.
