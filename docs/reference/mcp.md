# MCP endpoint

`POST /api/v1/mcp` exposes eval history to [Model Context Protocol](https://modelcontextprotocol.io)
clients — Claude Code, Claude Desktop, or any other MCP-speaking agent — so they can answer
"which cases regressed and why" against your data.

It is **opt-in**. Start the server with `DOMARINN_MCP_ENABLED=true`; without it the route is never
mounted and requests fall through to the ordinary JSON `404`.

```bash
DOMARINN_MCP_ENABLED=true DOMARINN_TOKENS="read:domarinn_agent" domarinn server

claude mcp add --transport http domarinn http://localhost:8321/api/v1/mcp \
  --header "Authorization: Bearer domarinn_agent"
```

## Authentication

The same credentials as the rest of the API, over `Authorization: Bearer` — a static token, an
account API key (`domarinn_…`), or a session token. Authorization is applied **per JSON-RPC
method**, not per route:

| Method | Scope |
|---|---|
| `server/discover`, `initialize`, `ping`, `notifications/*` | none |
| `tools/list`, `prompts/list`, `prompts/get` | none |
| `tools/call` | `read` |

Discovery is deliberately open: a client sends `server/discover` *before* it has credentials, and
a `401` there reads as a broken server rather than one that needs a token. The catalogs are
compiled in and identical for every caller, so they leak nothing. `tools/call` honors the usual
[`AuthMode`](./server.md#accounts--auth-model) — anonymous reads work in `open` and `protect-writes`.

A `401` carries `WWW-Authenticate: Bearer realm="domarinn"` with **no** `resource_metadata`
parameter. This server is not an OAuth resource server, and `/.well-known/oauth-protected-resource`
answers a definitive JSON `404` — some clients discard a configured static `Authorization` header
the moment a server advertises OAuth.

**Prefer a static token for agents.** `DOMARINN_TOKENS="read:…"` is verified in constant time
against memory and touches no database, whereas account API keys and sessions read (and
periodically refresh) a row per request.

## Tools

Eight read-only tools. None can start a run, mutate a baseline, or delete anything.

| Tool | Answers |
|---|---|
| `find_runs` | What projects, suites, and runs exist. `group_by` returns the catalog instead of runs; suite rows carry `baseline_run_id` and a pass-rate `series`. |
| `get_run` | One run's totals and provenance. `include: ["matrix", "config"]` embeds the prompt × provider grid and the config snapshot. |
| `list_cases` | A run's cases, filterable by status, cell, tag, error class, or free text. |
| `get_case` | One case in full: assertions with reasons, usage, cost, stop reason. Heavy fields (`raw`, `request`, `prompt`, `tool_calls`, `error_details`) require `fields`. |
| `case_history` | One case across the suite's recent runs — the flakiness question. |
| `compare_runs` | The diff between two runs, with McNemar and Wilson intervals. Defaults to changed cases only; widen with `delta`. |
| `search` | Full-text across runs and case content. |
| `get_server_info` | Instance version, accepted schema versions, auth mode, and (with `include: ["cache"]`) cache health. |

`export_run`, `cache_get`, and `cache_head` are deliberately absent: the lossless run document is a
context-window hazard that `get_run` + `list_cases` + `get_case` already covers, and cache blobs
are opaque `sha256:`-keyed bytes.

Three prompts — `triage_regression`, `investigate_case`, `summarize_run` — render instructions
naming the tools to call, and read no storage.

## Response limits

Tool results land directly in an agent's context window, so the MCP limits are far tighter than the
REST ones and truncation is never silent:

- Strings are cut at 2,000 characters with an explicit `…[truncated N of M chars]` marker and
  listed under `_truncated`. `get_case` accepts `max_chars` (up to 20,000) to widen.
- Row windows are small: 10 runs, 20 cases, 25 matrix rows, 50 compare rows by default.
- A serialized result over 64 KiB is refused with an actionable `isError`, never a truncated JSON
  body.
- `tools/call` is rate-limited per identity: 60/minute, burst 10, answered with `429` and
  `Retry-After`. Holders of the same static token share one bucket.

## Untrusted content

Stored model outputs are produced by the system under evaluation and, in a security-evaluation
suite, are adversarial by design. Before any of it reaches an agent, domarinn strips ANSI escape
sequences and control characters, replaces the FTS highlight sentinels, wraps the payload in an
`<untrusted source="stored_model_output">` fence, and neutralizes any closing marker inside it. The
server instructions and every prompt tell the model to treat fenced content as data, never as
instructions.

## Protocol

Dual-era, on one endpoint:

- **`2026-07-28`** (modern) — stateless. Requires `MCP-Protocol-Version`, `Mcp-Method`, and (where
  applicable) `Mcp-Name` headers, validated against the body; a mismatch is `400` with JSON-RPC
  `-32020`. Results carry `resultType` and, where cacheable, `ttlMs` + `cacheScope: "public"`.
- **`2025-11-25` / `2025-06-18`** (legacy) — the `initialize` handshake. No session id is ever
  minted or echoed, and `Mcp-Session-Id` / `Last-Event-ID` on a request are ignored.

Other behavior worth knowing: `GET`/`DELETE` answer `405` with `Allow: POST`; notifications answer
`202` with no body; an unknown method is `404` with `-32601`; the body limit is 256 KiB
*decompressed*; and responses are always `application/json` — this endpoint never emits an SSE
stream.

`Origin` is validated against a closed allowlist (`DOMARINN_PUBLIC_URL`,
`DOMARINN_MCP_ALLOWED_ORIGINS`, else loopback) to defeat DNS rebinding; an absent `Origin` is
allowed, since no CLI client sends one. CORS is layered on this route alone, with
`Access-Control-Allow-Credentials` off — so a cross-origin call can never be cookie-authenticated,
and the endpoint cannot become a CSRF vector.

## The Claude Code plugin

There is also a [Claude Code plugin](https://github.com/AtvikSecurity/domarinn/blob/main/plugin/README.md) that bundles this endpoint's
configuration along with skills for authoring and triaging suites.

```bash
claude plugin marketplace add AtvikSecurity/domarinn
claude plugin install domarinn@domarinn
```

<!-- This table is byte-identical to the "What you get" table in plugin/README.md
(the "8 MCP tools" table). Keep the two in sync; plugin/README.md is the source
of truth. -->
| Tool              | Answers                                                             |
| ----------------- | ------------------------------------------------------------------- |
| `find_runs`       | What projects, suites, and runs exist                               |
| `get_run`         | How one run went; which matrix cell is unhealthy                    |
| `list_cases`      | Which cases failed                                                  |
| `get_case`        | Why one case failed — assertions, stop reason, output               |
| `case_history`    | Is it flaky or newly broken                                         |
| `compare_runs`    | Did it get worse between two runs (with McNemar + Wilson intervals) |
| `search`          | Full-text across runs and case content                              |
| `get_server_info` | Instance version, accepted schema versions, cache health            |

Three prompts surface as slash commands — `/domarinn:triage`, `/domarinn:summarize`,
`/domarinn:check` — and two skills load on demand: `triage-evals` (reading eval history well) and
`author-evals` (writing suites, assertions, caching, and CI wiring). See plugin/README.md for the
full rundown.
