# domarinn CLI reference

One binary, `domarinn`: the eval engine, the results server, and its own
container healthcheck. Global flags apply to every subcommand.

```
domarinn [-v|-vv] [--server-url URL] <command> [args]
```

| Global flag | Effect |
|-------------|--------|
| `-v`, `-vv` | Increase log verbosity: `warn` (default) → `info` → `debug` → `trace`. Logs go to **stderr**. See [Logging](#logging). |
| `--log-format <fmt>` | How log lines are rendered: `auto` (default), `pretty`, `compact`, or `json` (also `DOMARINN_LOG_FORMAT`). See [Logging](#logging). |
| `--server-url <url>` | Results server base URL (or set `DOMARINN_SERVER_URL`). Used by `run --share`, `share`, and the `http` cache backend. |
| `--version` | Print the version. |
| `--help` | Print help for the binary or a subcommand. |

## Logging

Logs are diagnostics, not results: **they go to stderr**, while command output
(tables, JSON, JSONL, JUnit, schemas) goes to **stdout**. You can redirect or
pipe stdout for a machine — `domarinn run --format json > out.json` — and still
watch progress and warnings on the terminal.

**Verbosity.** A plain invocation logs at `warn`. Each `-v` raises the level one
step: `-v` → `info`, `-vv` → `debug`, `-vvv` → `trace`. Only domarinn's own
crates are affected.

**Format** — `--log-format`, or the `DOMARINN_LOG_FORMAT` environment variable:

| Value | Rendering |
|-------|-----------|
| `auto` (default) | Pretty when stderr is a terminal; **compact** (single line) when it is not — e.g. a captured CI log. |
| `pretty` | Human-readable, multi-line. No timestamps — a short command's own output is the clock. |
| `compact` | Human-readable, single line, with timestamps. |
| `json` | One JSON object per line, for log aggregation. |

Precedence is flag → env → autodetect: `--log-format` wins over
`DOMARINN_LOG_FORMAT`, which wins over the terminal autodetection above. An
unrecognized env value is ignored with a warning and autodetection is used.

**`RUST_LOG` overrides everything.** When set, it **replaces the default filter
wholesale** (standard `tracing` env-filter syntax), which also makes `-v` a
no-op — so set the level you want directly:

```sh
RUST_LOG=domarinn=debug domarinn run .                # -v is ignored; RUST_LOG wins
RUST_LOG=domarinn_core::runner=trace domarinn run .   # one module at trace
```

**Color.** ANSI color is used only when stderr is a terminal, and is disabled
entirely when [`NO_COLOR`](https://no-color.org/) is set (to any value).

## Exit codes

The exit code is a **contract for CI** — it distinguishes "the model got worse"
from "the harness broke". `3` (infra) wins over `1` (assertion) when both occur.

| Code | Name | Meaning |
|------|------|---------|
| `0` | OK | Everything passed. |
| `1` | assertion | An assertion failed, or a run regressed against a `--against` baseline. |
| `2` | config/usage | Bad config or flags, or a suite that fails to load/validate. |
| `3` | infra | Infrastructure error — a provider crashed, a grader was missing/broke, a `--cache-only` miss, the server was unreachable. |

---

## `domarinn run [PATH] [flags]`

Execute a suite: render prompts, call providers, evaluate assertions, report
results, and persist the run under `.domarinn/runs/<id>/`.

- `PATH` — a suite file or a directory containing `domarinn.yaml` (default `.`).

| Flag | Effect |
|------|--------|
| `--tag <T>` | Only run tests with this tag (repeatable; OR within tags, AND across kinds). |
| `--filter <GLOB>` | Only run tests whose id matches this glob (repeatable). |
| `--provider <ID>` | Only run this provider (repeatable). |
| `--prompt <ID>` | Only run this prompt (repeatable). |
| `--no-cache` | Never read or write the cache. |
| `--cache-only` | Read cache only; a miss is an infrastructure error (offline CI). |
| `--repeat <N>` | Run each cell N times (variance / pass@k). |
| `-j`, `--concurrency <N>` | Max concurrent provider calls (overrides `runner.concurrency`). |
| `--format <F>` | Output format, repeatable: `table` (default), `json`, `jsonl`, `junit`. |
| `--out <FILE>` | Write the primary output to a file instead of stdout. |
| `--against <REF>` | Compare against a baseline run (a run id, a `result.json` path, or `latest`); a regression sets exit code `1`. |
| `--summary-md <FILE>` | Write a Markdown summary (pass/fail counts, Wilson pass-rate CI, and any baseline comparison) — used by the CI action for PR comments. |
| `--share` | Upload the completed run to the configured server. |

```sh
domarinn run examples/render-health                 # run, print a table
domarinn run --tag safety -j 8 --format junit --out results.xml
domarinn run --against latest --summary-md summary.md
```

See [configuration.md](./configuration.md) for the suite file and
[statistics.md](./statistics.md) for `--repeat`/`--against`.

## `domarinn validate [PATH]`

Parse and structurally validate a suite. **No provider calls.** Use it in
pre-commit and CI to catch config errors fast. Exit `0` when valid (prints a
one-line summary); exit `2` and lists issues otherwise.

```sh
domarinn validate examples/render-health
```

## `domarinn diff <BASE> <HEAD> [--format table|json|md]`

Diff two runs. Each run reference is a run id, a `result.json` path, or `latest`.
Reports regressions, fixes, output changes, added/removed cases, and a McNemar
significance verdict. Exit `1` when `HEAD` regressed against `BASE`; `0`
otherwise. `--format md` emits a Markdown table for PR comments.

```sh
domarinn diff .domarinn/runs/A/result.json latest
```

## `domarinn view [RUN] [--format ...]`

Render a stored run in the terminal (default `latest`). `RUN` is a run id, a
`result.json` path, or `latest`. Formats: `table` (default), `json`, `jsonl`,
`junit`.

```sh
domarinn view latest
```

## `domarinn share [PATH] [--strict]`

Upload a completed run to a server and print its view URL. Enriches the run with
git and CI metadata automatically. Best-effort by default (a failed upload warns
and exits `0`); `--strict` makes upload failure fail the command (exit `3`).

- `PATH` — a `result.json`, a run directory, or omitted for the latest run.
- Server from `--server-url` / `DOMARINN_SERVER_URL`; token from
  `DOMARINN_TOKEN`.

```sh
DOMARINN_SERVER_URL=https://evals.example domarinn share --strict
```

## `domarinn cache <stats|path|gc|clear>`

Manage the local content-addressed response cache.

- `cache stats` — entry count and total size.
- `cache path` — print the cache directory (`.domarinn/cache`).
- `cache gc --older-than <30d|12h|45m|90s>` — remove entries older than a duration.
- `cache clear` — remove all entries.

See [caching.md](./caching.md) for backends and team sharing.

## `domarinn import promptfoo <PATH>`

Translate a promptfoo config into a domarinn suite, printed to stdout.
Mappable providers, prompts, tests, and assertions are converted; anything
without a faithful equivalent is emitted as a commented `# NOTE:` line so nothing
is silently dropped.

```sh
domarinn import promptfoo promptfooconfig.yaml > domarinn.yaml
domarinn validate domarinn.yaml
```

## `domarinn gen-types [DIR]`

Generate TypeScript type definitions for the result and diff DTOs (default
`web/src/api/generated`). These are the single source of truth for the web
client's types; CI enforces they stay in sync.

## `domarinn schema <config|result>`

Print a JSON Schema to stdout — for editor completion and as a CI contract.
`config` is the suite schema; `result` is the `RunResult` schema. The checked-in
`domarinn.schema.json` is regenerated from `schema config` and CI fails on
drift.

```sh
domarinn schema config > domarinn.schema.json
```

## `domarinn list <tests|providers|prompts> [PATH] [--json]`

List what a suite resolves to (tests are fully resolved — globs and generators
included). `--json` emits a JSON array.

```sh
domarinn list providers examples/render-health
domarinn list tests . --json
```

## `domarinn server [--port N] [--data-dir DIR]`

Run the self-hostable results server + embedded web UI (default port `8321`,
binds `0.0.0.0`; default data dir `/data`, env `DOMARINN_DATA_DIR`). See
[server.md](./server.md) and [deploy.md](./deploy.md).

```sh
domarinn server --port 8321 --data-dir ./data
```

## `domarinn healthcheck [--port N]`

Probe **this binary's own** server health and exit `0`/non-zero accordingly.
Designed for the container `HEALTHCHECK` in the distroless image, which has no
shell or curl.

---

## CI usage

- **Validate on every push:** `domarinn validate` (fast, no provider calls).
- **Gate PRs on eval quality:** `domarinn run --against latest` (exit `1` on
  regression), or use the reusable action at `.github/actions/domarinn-eval`.
  See [ci.md](./ci.md).
- **Contract-test the schema:** regenerate `domarinn schema config` and fail
  on drift (wired in `ci.yml`).
- **Read the exit code**, not just stdout: `1` = the model regressed (block the
  PR), `3` = the harness broke (retry / page an operator, don't blame the PR).
