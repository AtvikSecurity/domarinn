<!-- Ownership: operating semantics (auth model, credential kinds, env vars,
storage, logging) live in reference/server.md. Procedures (getting it running:
Docker, compose, Kubernetes, proxies, backups, upgrades, first admin) live in
guides/self-host.md. The per-view UI walkthrough lives in reference/web-ui.md.
The env-var table exists ONLY in reference/server.md. -->

# The domarinn server & accounts

/// info | This page split
The REST API moved to [REST API](./rest-api.md), the MCP endpoint moved to
[MCP endpoint](./mcp.md), and the UI walkthrough moved to
[The web UI](./web-ui.md). This page keeps operating the server itself: accounts, auth,
environment variables, logging, and storage. For Docker/Kubernetes/backups, see
[Self-hosting](../guides/self-host.md).

///

`domarinn server` runs the results server: a JSON API under `/api/v1` **and** the embedded React web UI, served from the **same binary**. There is no separate frontend to deploy, no sidecar database, and no runtime dependency — the binary is the eval engine, the CLI, the server, and its own container healthcheck.

```sh
domarinn server [--port 8321] [--data-dir /data]
```

| Flag         | Default | Effect |
|--------------|---------|--------|
| `--port`     | `8321`  | Listen port. The server always binds `0.0.0.0`. |
| `--data-dir` | `/data` | State directory (also `DOMARINN_DATA_DIR`). Holds the SQLite databases. |

The server runs until Ctrl-C (graceful shutdown). Health is exposed at both `/health` and `/api/v1/health`. See [`cli.md`](./cli.md) for the rest of the binary's subcommands and [Self-hosting](../guides/self-host.md) for Docker/Kubernetes hosting.

- [Quick start](#quick-start)
- [Accounts & auth model](#accounts--auth-model)
- [Single sign-on (OIDC & SAML)](#single-sign-on-oidc--saml)
- [Strict request validation](#strict-request-validation)
- [Environment variables](#environment-variables)
- [Logging & observability](#logging--observability)
- [Storage](#storage)
- [Web UI](#web-ui)

---

## Quick start

```sh
# Closed mode (the default) — every page and API call requires a login.
domarinn server --data-dir ./data
# UI + API on http://localhost:8321 — the first visit walks you through
# creating the admin account (or POST /api/v1/auth/setup).
```

See [Self-hosting](../guides/self-host.md) for Docker, Compose, Kubernetes, bootstrapping the
admin and a CI token from the environment, and creating the first admin.

---

## Accounts & auth model

domarinn has **three auth modes** and is **`closed` by default** — anonymous access is always an explicit operator choice, never an inferred one.

| Mode             | Reads / UI | Writes (ingest, baseline, cache PUT) | Admin (delete, prune, users) |
|------------------|------------|--------------------------------------|------------------------------|
| `open`           | open       | open                                 | open                         |
| `protect-writes` | open       | require `write`                      | require `admin`              |
| `closed`         | require `read` | require `write`                  | require `admin`              |

**How the mode is chosen (at startup):**

1. If `DOMARINN_AUTH_MODE` is set (`open` \| `protect-writes` \| `closed`), it wins outright. (`protect_writes` with an underscore is also accepted.)
2. Otherwise the mode is **`closed`**. Nothing is derived from whether credentials exist.

Set `DOMARINN_AUTH_MODE=open` to explicitly opt out of auth entirely — the usual choice for a
laptop or a trusted network.

Even in `closed` mode the bootstrap surface stays reachable so a fresh install can be claimed: `/health`, `GET /api/v1/meta`, `POST /api/v1/auth/setup` (one-shot, while zero users exist), `login`, `me`, and the web UI shell (the app itself redirects to the login page). The active mode is reported by `GET /api/v1/meta` as `auth_mode`.

> **Upgrade note.** Older releases derived `protect-writes` from the presence of
> tokens or accounts and defaulted to `open` otherwise. A deployment that never
> set `DOMARINN_AUTH_MODE` now comes up `closed`: CI uploads with a `write`
> token keep working unchanged, but anonymous *reads* now require a `read`
> token (or a login). Set `DOMARINN_AUTH_MODE=protect-writes` to restore the
> old behavior.

> **`DOMARINN_AUTH_MODE` is validated at startup.** An unrecognized value (a
> typo like `protectwrites`) **aborts the launch with an error** rather than
> silently falling back to `open` — a silent downgrade to a wide-open server
> would be a security hole. The accepted values are `open`, `protect-writes` (the
> `protect_writes` underscore spelling is also taken), and `closed`.

> Scopes are ordered: **`admin` ⊃ `write` ⊃ `read`**. A route asks for a minimum
> scope; a higher scope always satisfies it. In `open` mode no scope is required
> at all; in `protect-writes` the `read` requirement is waived but `write`/`admin`
> are enforced; in `closed` every requirement is enforced.

### Two kinds of credentials

Both are presented the same way — as a bearer token in the `Authorization` header (see [The auth header](#the-auth-header)).

**1. Static bearer tokens** — configured via the environment, no database rows, no user. Ideal for CI and bootstrapping.

```
DOMARINN_TOKENS="read:domarinn_view,write:domarinn_ci,admin:domarinn_ops"
```

Each comma-separated entry is `scope:secret`, where scope is `read`, `write`, or `admin`. The secret string is whatever you choose (the `domarinn_` names above are illustrative). Static tokens are matched in constant time and are **not** tied to a user account — so they cannot create API keys or appear in the users list.

**2. Local user accounts** — real username/password logins stored in SQLite.

- Passwords are **argon2**-hashed (minimum 8 characters).
- Two roles: **`admin`** and **`member`**. Role maps to a scope ceiling: `admin → admin`, `member → write`.
- Logging in mints a **session** (token prefix `mses_`, 30-day lifetime). The browser UI uses sessions; `POST /auth/logout` revokes the presenting one.
- Each account can mint **API keys** (prefix `domarinn_`, 256 bits of entropy). The secret is shown **exactly once** on creation, is revocable, and carries a **scope ceiling** — a key may be created at or below the creator's own scope, never above it.

| Credential   | Prefix   | Backed by | Can manage accounts/keys? | Typical use |
|--------------|----------|-----------|---------------------------|-------------|
| Static token | (any)    | env var   | no                        | CI, bootstrap |
| Session      | `mses_`  | account   | yes (as the user)         | Web UI login |
| API key      | `domarinn_`  | account   | yes (as the user)         | Scripts, CI tied to a user |

The authenticator chain resolves a presented token in order — **static token → API key → session** — dispatching account lookups by prefix so at most one DB hit occurs per request.

### The auth header

```
Authorization: Bearer <static-token | domarinn_apikey | mses_session>
```

The `Bearer ` prefix is recommended; a bare token value in the header is also accepted. The same header works for every credential kind — the server figures out which one you presented.

---

## Single sign-on (OIDC & SAML)

domarinn can delegate login to one or more external identity providers — any OIDC provider (Google, Authentik, Okta, Entra, Keycloak, …) and any SAML 2.0 IdP. SSO is configured **entirely through the environment**; each configured provider becomes a "Continue with …" button on the login page.

**How it works.** A first SSO login **just-in-time provisions** a local account, matched strictly on the provider + IdP subject (never on email). The account's role is mapped from the IdP's group/claim data and **re-synced on every SSO login**, so the IdP stays the source of truth — with one exception: the last enabled admin is never auto-demoted. SSO-only accounts have no password and cannot use the password form. Browser sessions ride a secure, HttpOnly cookie; `Authorization: Bearer` (API keys, static tokens, the CLI) is unaffected.

> `DOMARINN_PUBLIC_URL` **must** be set whenever any provider is configured —
> it builds the OIDC redirect URI (`{PUBLIC_URL}/api/v1/auth/oidc/<name>/callback`)
> and the SAML ACS/entity URLs. Register that redirect URI at your IdP.

### Global SSO settings

| Variable | Default | Purpose |
|---|---|---|
| `DOMARINN_SSO_CLOCK_SKEW_SECS` | `60` | Tolerance for OIDC `exp`/`iat` and SAML `NotBefore`/`NotOnOrAfter`. |

### OIDC providers

List the provider names, then set per-provider variables (`<NAME>` is the name uppercased with `-`→`_`):

```
DOMARINN_OIDC_PROVIDERS=google,authentik
DOMARINN_OIDC_<NAME>_ISSUER=https://accounts.google.com        # required
DOMARINN_OIDC_<NAME>_CLIENT_ID=...                             # required
DOMARINN_OIDC_<NAME>_CLIENT_SECRET=...                         # required
DOMARINN_OIDC_<NAME>_LABEL=Google                              # button label (default: capitalized name)
DOMARINN_OIDC_<NAME>_SCOPES=openid email profile               # default shown
DOMARINN_OIDC_<NAME>_GROUPS_CLAIM=groups                       # ID-token claim holding groups
DOMARINN_OIDC_<NAME>_ADMIN_GROUPS=platform-admins,sec-ops      # membership → admin
DOMARINN_OIDC_<NAME>_ADMIN_EMAILS=ops@example.com              # or map admins by email
DOMARINN_OIDC_<NAME>_ALLOWED_EMAIL_DOMAINS=example.com         # restrict who may sign in (optional)
```

> **Google** does not expose a groups claim — use `ADMIN_EMAILS` for admin
> mapping. **Authentik/Keycloak/Okta** emit `groups` (configurable via the
> claim name). When `ALLOWED_EMAIL_DOMAINS` is set, an IdP-unverified email is
> rejected rather than trusted.

### SAML providers

SAML requires the binary to be built **with the `saml` cargo feature** (the published Docker image is; a plain `cargo build` is not, and a SAML-configured binary without the feature hard-errors at startup). Configure exactly one IdP source per provider:

```
DOMARINN_SAML_PROVIDERS=okta
# one of the three IdP sources:
DOMARINN_SAML_<NAME>_IDP_METADATA_URL=https://…/metadata        # fetched at startup
DOMARINN_SAML_<NAME>_IDP_METADATA_FILE=/etc/domarinn/okta.xml    # or read from a file
DOMARINN_SAML_<NAME>_IDP_SSO_URL=…  + DOMARINN_SAML_<NAME>_IDP_CERT=<PEM>   # or explicit
DOMARINN_SAML_<NAME>_SP_ENTITY_ID=…                              # default: the SP metadata URL
DOMARINN_SAML_<NAME>_LABEL=Okta
DOMARINN_SAML_<NAME>_EMAIL_ATTR=email                           # default: emailAddress NameID, else email/mail
DOMARINN_SAML_<NAME>_GROUPS_ATTR=groups
DOMARINN_SAML_<NAME>_ADMIN_GROUPS=…  / _ADMIN_EMAILS=…  / _ALLOWED_EMAIL_DOMAINS=…
DOMARINN_SAML_<NAME>_ALLOW_IDP_INITIATED=false                  # require InResponseTo unless true
```

The SP metadata your IdP imports is served at `{PUBLIC_URL}/api/v1/auth/saml/<name>/metadata`. Response signatures are verified (RSA/ECDSA SHA-2 only); **encrypted assertions are not supported** — disable assertion encryption for the domarinn app at your IdP. IdP metadata without a signing certificate is refused at startup.

Startup **fails fast** on any misconfiguration (a missing required variable is named exactly, an unreachable/invalid SAML metadata source aborts the launch). OIDC discovery itself is lazy, so a temporarily-unreachable OIDC IdP does not prevent the server from starting.

---

## Strict request validation

The API rejects malformed requests loudly instead of quietly guessing. A typo in a query string or a stale field name fails fast with a clear status, rather than being silently ignored and masking the mistake.

- **Unknown query parameters, and unparseable filter values, are `400`.** An unrecognized value for `?status=` on `GET /runs` or `GET /runs/{id}/cases` is a `400`, and so is a query string carrying a parameter the endpoint does not define. The two `status` filters are deliberately different: the case filter accepts `pass | fail | error | skip`, but the **run-level** filter accepts only `pass | fail | error`. A skipped case never moves a run's pass/fail/error counters, so `GET /runs?status=skip` is a `400` — not an empty result set. Likewise, `POST /cache/prune` takes `older_than_days` and `target_bytes` as **query** parameters, so an unknown param there is a `400` as well.
- **Unknown fields in a JSON request body are `422`.** A misspelled or stray key (in a user, API-key, or baseline body) is rejected rather than dropped — as is a value of the wrong type or an unrecognized enum value (any body that parses as JSON but does not match the target shape). Syntactically invalid JSON, or a missing/incorrect `Content-Type`, is a `400`.
- **An unrecognized assertion `kind` in an ingested run document is `422`.** `POST /api/v1/runs` deserializes the body into the strict `RunResult` schema, so an unknown assert kind — or any other unknown field or out-of-range value — fails validation instead of being stored as-is. (A body outside the supported `schema_version` window is also `422`; see [ingest](./rest-api.md#run-ingest).)

---

## Environment variables

**Read by the server** (`domarinn server`):

| Variable                        | Default        | Purpose |
|---------------------------------|----------------|---------|
| `DOMARINN_DATA_DIR`           | `/data`        | State directory. Holds `domarinn.db` and `cache.db`. Also settable with `--data-dir`. |
| `DOMARINN_TOKENS`             | (unset)        | Static bearer tokens as `scope:secret` pairs, comma-separated. Grants access but never changes the mode. |
| `DOMARINN_AUTH_MODE`          | `closed`       | The mode: `open` \| `protect-writes` \| `closed`. Unset means `closed`. |
| `DOMARINN_ADMIN_USER`         | (unset)        | Bootstrap admin username. Requires the password too. |
| `DOMARINN_ADMIN_PASSWORD`     | (unset)        | Bootstrap admin password. The account is (re)ensured on every startup. |
| `DOMARINN_PUBLIC_URL`         | (unset)        | Public base URL for share links / absolute URLs. No trailing slash, no path prefix. **Required when any SSO provider is configured** (redirect URIs / SAML endpoints). |
| `DOMARINN_COOKIE_SECURE`      | (from URL)     | Force the session cookie's `Secure` flag `true`\|`false`. Defaults to on when `DOMARINN_PUBLIC_URL` is `https://`. |
| `DOMARINN_CACHE_MAX_ENTRY_BYTES` | `4194304` (4 MiB)   | Max size of a single cache entry; a larger `PUT` gets a `413`. Since 0.5.0 an entry carries the request it answers and, for a grading, the grading model's whole response, so entries are bigger than they were — an oversized one is logged by the client and re-paid on every run rather than failing it. Raise this before lowering what a suite sends. |
| `DOMARINN_CACHE_MAX_BYTES`    | `1073741824` (1 GiB) | Total cache size target for retention. |
| `DOMARINN_CACHE_MAX_AGE_DAYS` | `30`           | Cache entry max age for retention. |
| `DOMARINN_RUN_MAX_AGE_DAYS`   | *(unset)*      | Delete runs older than this many days. **Unset means never delete** — eval history is expensive to produce and impossible to recreate, so retention is opt-in. Two runs are exempt at any age: a pinned baseline (deleting it breaks `--against server:baseline`) and the newest run of each `(project, suite, branch)` (a suite that has not run in a while must go stale, not vanish). Swept hourly, alongside cache retention. |
| `DOMARINN_MCP_ENABLED`        | `false`        | Mount the [MCP endpoint](./mcp.md) at `POST /api/v1/mcp`. Opt-in. A value other than `true`\|`false`\|`1`\|`0`\|`yes`\|`no` is a startup error, not a silent `false`. |
| `DOMARINN_MCP_ALLOWED_ORIGINS`| (unset)        | Extra origins allowed to reach the MCP endpoint, comma-separated. Accepts full origins (`https://app.example`) or bare `host[:port]`; an entry without a port matches any port. Drives both the `Origin` check and the route's CORS layer. With this and `DOMARINN_PUBLIC_URL` both unset, only loopback origins are allowed. |
| `DOMARINN_LOG_FORMAT`         | (auto)         | Log rendering: `pretty` \| `compact` \| `json`. Auto-selected from the terminal when unset — see [Logging & observability](#logging--observability). |
| `RUST_LOG`                      | (unset)        | Overrides the default log filter wholesale, e.g. `RUST_LOG=domarinn=debug,tower_http=off`. When unset the server logs at `info`. Logs go to stderr. |

**Read by the CLI** (when uploading runs / using an HTTP cache — *not* the server):

| Variable                | Purpose |
|-------------------------|---------|
| `DOMARINN_SERVER_URL` | Target server base URL for `domarinn run --share` / `share` (or the `--server-url` flag). |
| `DOMARINN_TOKEN`      | A single bearer token the CLI sends when uploading a run or using the HTTP cache backend. |

See [`cli.md`](./cli.md) and [`../concepts/caching.md`](../concepts/caching.md) for the client side.

---

## Logging & observability

The server logs to **stderr** at `info` by default. API responses go over the wire, never into the log stream, so structured logs stay clean for aggregation.

**Request logging.** Every HTTP request produces an `http` span carrying its `method`, `path`, and `request_id`, and a `response` event carrying the `status` and `latency_ms`. Schematically, one request reads:

```
INFO http{method=GET path=/api/v1/runs request_id=01J...}: response status=200 latency_ms=7
```

**Request ids.** If a request arrives with an `x-request-id` header, that id is honored and threaded through its log line; otherwise the server mints a ULID. In both cases the id is echoed back on the response's `x-request-id` header, so a client or a proxy can correlate a single call end to end.

**Format.** As on the CLI, rendering is auto-selected: **pretty** when stderr is a terminal, and **JSON** (one object per line) when it is not — which is the usual case under Docker/Kubernetes, so container logs are structured out of the box. Force a format with `DOMARINN_LOG_FORMAT=pretty|compact|json`.

**Turning the volume down.** `RUST_LOG` replaces the default filter entirely. To keep warnings and errors but silence the per-request `info` lines:

```sh
RUST_LOG=domarinn=warn,tower_http=off domarinn server
```

> Known consideration: the container healthcheck probes `/health` on an interval,
> so at the default `info` level you will see periodic `GET /health` request lines
> — the `RUST_LOG` filter above removes all request logging if that noise is
> unwelcome.

---

## Storage

State lives in the data directory as **two SQLite files** (WAL mode):

| File            | Contents | Back up? |
|-----------------|----------|----------|
| `domarinn.db` | Durable run history. Each run is stored both as a compressed lossless blob (for export) and as normalized rows for indexed filtering. Also holds users, sessions, API keys, and baselines. | **Yes — this is the backup target.** |
| `cache.db`      | The content-addressed request cache. Regenerable. | No — disposable. |

SQLite is a **single writer**: run exactly one instance against a given data directory. This is what makes backups a file copy and self-hosting a one-liner — and why the deployment guidance is single-replica. Migrations run automatically at startup. See [Self-hosting](../guides/self-host.md) for backup and Kubernetes details.

---

## Web UI

The UI is a single-page app served at the web root out of this same binary (client-side routing; give the service its own hostname, not a path prefix).

Its routes, and a screenshot walkthrough of every view, are [The web UI](./web-ui.md).

