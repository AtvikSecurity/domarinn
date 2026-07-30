# domarinn CLI reference

One binary, `domarinn`: the eval engine, the results server, and its own container healthcheck. Global flags apply to every subcommand.

```
domarinn [-v|-vv] [--color WHEN] [--server-url URL] <command> [args]
```

| Global flag | Effect |
|-------------|--------|
| `-v`, `-vv` | Increase log verbosity: `warn` (default) → `info` → `debug` → `trace`. Logs go to **stderr**. See [Logging](#logging). |
| `--log-format <fmt>` | How log lines are rendered: `auto` (default), `pretty`, `compact`, or `json` (also `DOMARINN_LOG_FORMAT`). See [Logging](#logging). |
| `--color <when>` | When to colorize human output (the results table, listings, diffs): `auto` (default; color a TTY), `always`, or `never`. Honors [`NO_COLOR`](https://no-color.org/) and `CLICOLOR_FORCE`. **Machine formats (`json`/`jsonl`/`junit`) are never colored**, and neither is `--out` file output. |
| `--server-url <url>` | Results server base URL (or set `DOMARINN_SERVER_URL`). Used by `run --share`, `share`, and the `http` cache backend. |
| `--version` | Print the version. |
| `--help` | Print help for the binary or a subcommand. |

## Logging

Logs are diagnostics, not results: **they go to stderr**, while command output (tables, JSON, JSONL, JUnit, schemas) goes to **stdout**. You can redirect or pipe stdout for a machine — `domarinn run --format json > out.json` — and still watch progress and warnings on the terminal.

**Verbosity.** A plain invocation logs at `warn`. Each `-v` raises the level one step: `-v` → `info`, `-vv` → `debug`, `-vvv` → `trace`. Only domarinn's own crates are affected.

**Format** — `--log-format`, or the `DOMARINN_LOG_FORMAT` environment variable:

| Value | Rendering |
|-------|-----------|
| `auto` (default) | Pretty when stderr is a terminal; **compact** (single line) when it is not — e.g. a captured CI log. |
| `pretty` | Human-readable, multi-line. No timestamps — a short command's own output is the clock. |
| `compact` | Human-readable, single line, with timestamps. |
| `json` | One JSON object per line, for log aggregation. |

Precedence is flag → env → autodetect: `--log-format` wins over `DOMARINN_LOG_FORMAT`, which wins over the terminal autodetection above. An unrecognized env value is ignored with a warning and autodetection is used.

**`RUST_LOG` overrides everything.** When set, it **replaces the default filter wholesale** (standard `tracing` env-filter syntax), which also makes `-v` a no-op — so set the level you want directly:

```sh
RUST_LOG=domarinn=debug domarinn run .                # -v is ignored; RUST_LOG wins
RUST_LOG=domarinn_core::runner=trace domarinn run .   # one module at trace
```

**Color.** ANSI color is used only when stderr is a terminal, and is disabled entirely when [`NO_COLOR`](https://no-color.org/) is set (to any value).

## Exit codes

The exit code is a **contract for CI** — it distinguishes "the model got worse" from "the harness broke". `3` (infra) wins over `1` (assertion) when both occur.

| Code | Name | Meaning |
|------|------|---------|
| `0` | OK | Everything passed. |
| `1` | assertion | An assertion failed, or a run regressed against a `--against` baseline. |
| `2` | config/usage | Bad config or flags, a suite that fails to load or validate, a run that resolved to **zero cases**, or a missing / wrong-shaped provider credential. |
| `3` | infra | Infrastructure error — a provider crashed, a grader was missing/broke, the server was unreachable, or a `--cache-only` run could not answer honestly (a miss, or a case whose `latency` assertion always needs a live call). |

---

## `domarinn run [PATH] [flags]`

Execute a suite: render prompts, call providers, evaluate assertions, report results, and persist the run under `.domarinn/runs/<id>/`.

- `PATH` — a suite file or a directory containing `domarinn.yaml` (default `.`).

| Flag | Effect |
|------|--------|
| `--tag <T>` | Only run tests with this tag (repeatable; OR within tags, AND across kinds). |
| `--filter <GLOB>` | Only run tests whose id matches this glob (repeatable). |
| `--provider <ID>` | Only run this provider (repeatable). |
| `--prompt <ID>` | Only run this prompt (repeatable). |
| `--no-cache` | Never read or write the cache. |
| `--cache-only` | Read cache only; a miss is an infrastructure error (offline CI). The credential preflight is skipped, and a case carrying a `latency` assertion is refused rather than called live — see [caching.md](../concepts/caching.md#cache-modes). |
| `--no-grader-cache` | Grader-originated requests (the LLM grader, `exec` graders, embeddings) bypass the cache; responses of the systems under test are still replayed. Use it to measure grader variance deliberately. Replaces the deprecated suite key `cache.grader: false`. |
| `--cache-dir DIR` | Where the local cache lives. Defaults to `.domarinn/cache` beside the suite, so the same suite hits the same cache from any directory. `DOMARINN_CACHE_DIR` sets the same thing; the flag wins. |
| `--no-cache-migration` | Skip looking for entries written under an older cache-key shape. domarinn probes for those on a miss so an upgrade does not discard a warm cache, and stops once it is clear there is nothing to find. |
| `--repeat <N>` | Run each cell N times (variance / pass@k). |
| `-j`, `--concurrency <N>` | Max concurrent provider calls (overrides `runner.concurrency`). |
| `--format <F>` | Output format, repeatable: `table` (default), `json`, `jsonl`, `junit`, `md`. `md` is the same run-summary Markdown as `--summary-md`, on stdout. |
| `--out <FILE>` | Write the primary output to a file instead of stdout. |
| `--no-raw` | Do not persist raw provider metadata in the result document (keeps `result.json` small). The prompt and `stop_reason` are still captured. |
| `--no-progress` | Disable the live progress bar (see below). |
| `--allow-empty` | Succeed even if the run resolves to zero cases. Without it that is exit 2, because a green result over no cells is indistinguishable from a green result over every cell. Pass it for a sharded matrix where a shard legitimately has no work. |
| `--against <REF>` | Compare against a baseline run. `server:baseline` uses the baseline pinned for this suite on the results server (the only reference that works in CI); `latest` uses the newest local run *of the same suite*; also accepts a run id or a `result.json` path. A regression sets exit code `1`; a baseline that was requested but could not be resolved sets exit code `2`. |
| `--summary-md <FILE>` | Write a Markdown summary (headline metrics table, failing cases, and any baseline comparison). Identical to what [`ci-summary`](#domarinn-ci-summary-run-flags) writes, minus the step outputs. |
| `--share` | Upload the completed run to the configured server, and record the returned URL on the stored run. |
| `--note <TEXT>` | A short human label for this run ("trying temperature 0.3"). Stored on the run and full-text searchable on the server. Defaults to the suite's `description`. |
| `--no-provenance` | Do not record the OS username or hostname. Git, CI and version metadata are still recorded, and the run is marked redacted. |

```sh
domarinn run examples/12-render-health                 # run, print a table
domarinn run --tag safety -j 8 --format junit --out results.xml
domarinn run --against server:baseline --summary-md summary.md
domarinn run --note "retry backoff, 3rd attempt"    # label this run
```

### Run provenance

Every run records who and what produced it, in the engine — so a plain `domarinn run` carries it, not only a shared one:

| Field | Source |
|-------|--------|
| `origin.actor` | `DOMARINN_ACTOR`, else the CI actor (`GITHUB_ACTOR`, `GITLAB_USER_LOGIN`, …), else the OS username. In CI the CI actor wins because the OS user there is a service account that identifies nobody. |
| `origin.host` | `DOMARINN_HOST`, else the hostname. |
| `origin.version` | The domarinn build that produced the document. |
| `origin.note` | `--note`, else the suite's `description`. |
| `git` | Branch, commit and dirty state of the repo containing the suite. |
| `ci` | The detected CI system and its run URL. `ci` being present *is* the "was this CI?" flag — there is no separate boolean to disagree with it. |

**Suppressing it.** `actor` and `host` are mild PII, and once written they are inside the document the server content-hashes for ingest idempotency, so they cannot be redacted afterwards without changing that hash. Suppression therefore has to happen on the client:

| Setting | Effect |
|---------|--------|
| `DOMARINN_PROVENANCE=full` | The default: record everything. |
| `DOMARINN_PROVENANCE=anonymous` | Drop `actor`/`host`; keep git, CI, version and note. Sets `origin.redacted: true` so a reader can tell suppression from an older client. |
| `DOMARINN_PROVENANCE=off` | Record nothing — no `origin`, `git` or `ci` key at all. |
| `--no-provenance` | Same as `anonymous`. It can only *tighten* the environment's policy, never re-enable what the environment turned off. |

Set the environment variable in the image or on the machine when this is an organisation-wide policy; the flag is for one-off runs.

**Live progress.** When stderr is a terminal, `run` draws a single progress bar on **stderr** — elapsed time, a bar, `done/total`, and a running pass/fail/error tally. It is purely cosmetic: it never touches **stdout**, so piping or redirecting results (`domarinn run --format json > out.json`) is byte-identical with or without it. The bar is suppressed automatically when stderr is not a terminal (e.g. captured CI logs), under `-vv`+ (so it never clobbers streamed diagnostics), and with `--no-progress`.

See [domarinn.yaml](domarinn-yaml.md) for the suite file and [statistics.md](../concepts/statistics.md) for `--repeat`/`--against`.

## `domarinn validate [PATH]`

Parse and structurally validate a suite. **No provider calls.** Use it in pre-commit and CI to catch config errors fast. Exit `0` when valid (prints a one-line summary); exit `2` and lists issues otherwise.

```sh
domarinn validate examples/12-render-health
```

## `domarinn diff <BASE> <HEAD> [flags]`

Diff two runs. Each run reference is a run id, a `result.json` path, or `latest`. Reports regressions, fixes, output changes, added/removed cases, and a McNemar significance verdict. Exit `1` when `HEAD` regressed against `BASE`; `0` otherwise.

| Flag | Effect |
|------|--------|
| `--format <F>` | `table` (default), `json`, or `md` (a Markdown table for PR comments). |
| `--diffs <SCOPE>` | Which cases get an inline output diff: `regressions` (default; newly-failing only), `changed` (any case whose output changed), `all` (every joined case), or `none`. |
| `--full` | Do not truncate inline output diffs (show every changed line). |
| `--config-diff` | Diff the full config snapshot. Default is a compact digest note plus the prompts section only. |

```sh
domarinn diff .domarinn/runs/A/result.json latest
domarinn diff base latest --diffs all --full --config-diff
```

## `domarinn view [RUN] [flags]`

Render a stored run in the terminal (default `latest`). `RUN` is a run id, a `result.json` path, or `latest`.

| Flag | Effect |
|------|--------|
| `--format <F>` | `table` (default), `json`, `jsonl`, `junit`, `md`. |
| `--failed` | Show only failed/errored cases. The table footer still summarizes the whole run; `json`/`jsonl`/`junit` emit only the filtered cases. |
| `--case <SEL>` | Show full detail for matching case(s) — a `case_key`, a `case_key` prefix (≥4 chars), a test id, or a name substring (repeatable). Rejected for `junit`/`md`. |
| `--raw` | With `--case`, include the raw provider metadata (v2 runs). |

```sh
domarinn view latest
domarinn view latest --failed
domarinn view latest --case greet --raw
```

## `domarinn runs [flags]`

List stored runs, newest first — a local table by default, or the results server's runs with `--remote`.

| Flag | Effect |
|------|--------|
| `-n`, `--limit <N>` | Max runs to show (default `20`; `0` = unlimited). |
| `--suite <NAME>` | Only runs of this suite. |
| `--remote` | List runs from the results server (`--server-url` / `DOMARINN_SERVER_URL`) instead of `.domarinn/runs`. |
| `--json` | Emit JSON instead of a table. |

```sh
domarinn runs -n 5
domarinn runs --suite refusals --json
domarinn runs --remote
```

## `domarinn share [RUN] [--strict]`

Upload a completed run to a server and print its view URL. Enriches the run with git and CI metadata automatically. Best-effort by default (a failed upload warns and exits `0`); `--strict` makes upload failure fail the command (exit `3`).

- `RUN` — a run id from `domarinn runs`, `latest`, a `result.json`, a run directory, or omitted for the latest run (same references as `view` and `diff`).
- Server from `--server-url` / `DOMARINN_SERVER_URL`; token from `DOMARINN_TOKEN`.

```sh
DOMARINN_SERVER_URL=https://evals.example domarinn share --strict
DOMARINN_SERVER_URL=https://evals.example domarinn share 01JD3V9GQ8 --strict
```

## `domarinn ci-summary [RUN] [flags]`

Summarize a stored run for CI: a Markdown report for a PR comment or job summary, plus the headline numbers as GitHub Actions step outputs. See [`gate-in-ci.md`](../guides/gate-in-ci.md#the-ci-summary-command).

| Flag | Meaning |
|---|---|
| `RUN` | Run to summarize — a run id, `latest` (default), a `result.json`, or a run directory. |
| `--against <REF>` | Append a baseline comparison; same references as `run --against`. `ci-summary` is a reporter, not a gate, so an unresolvable baseline warns and is skipped rather than failing. |
| `--out <FILE>` | Write the Markdown to a file instead of stdout. |
| `--github-output <FILE>` | Append `key=value` step outputs here. Defaults to `$GITHUB_OUTPUT`, so on a runner no flag is needed. |

**It is a reporter, not a gate** — it exits `0` for a failing run, because the verdict belongs to `run`'s [exit code](#exit-codes). It exits `2` only when the run reference cannot be resolved.

```sh
domarinn ci-summary                              # latest run, Markdown on stdout
domarinn ci-summary --against server:baseline --out summary.md
```

## `domarinn cache <stats|path|gc|clear>`

Manage the **local** content-addressed cache. All four take a suite path (default `.`) and the same `--cache-dir` a run does, so they inspect the directory that run would actually use.

- `cache stats` — entry count and total size.
- `cache path` — print the cache directory (`.domarinn/cache` beside the suite).
- `cache gc --older-than <30d|12h|45m|90s>` — remove entries older than a duration. `--older-than` is **required**: a bare `gc` is a usage error (exit `2`), because the obvious reading of "gc" is "tidy up a bit" and the command that removes everything should be the one that says so.
- `cache clear` — remove all entries.

Two scope rules worth knowing:

- **Local tier only.** These never reach an S3 bucket or the server. Remote retention is the bucket's lifecycle rules and the server's [prune endpoint plus hourly retention task](rest-api.md#cache-shared-provider-cache).
- **The pre-0.4 legacy tier is reported always, purged only when it is yours.** A cwd-relative `.domarinn/cache` left by an older domarinn is shown by `stats` and `path`, but `clear`/`gc` touch it only when the suite sits at or under the current directory — `cd ~/projB && domarinn cache clear ~/projA/evals` must not take projB's cache with it. `stats` says which of the two applies.

See [caching.md](../concepts/caching.md) for the key rule, backends, and team sharing.

## `domarinn import promptfoo <PATH>`

Translate a promptfoo config into a domarinn suite, printed to stdout. Mappable providers, prompts, tests, and assertions are converted; anything without a faithful equivalent is emitted as a commented `# NOTE:` line so nothing is silently dropped.

```sh
domarinn import promptfoo promptfooconfig.yaml > domarinn.yaml
domarinn validate domarinn.yaml
```

## `domarinn gen-types [DIR]`

Generate TypeScript type definitions for the result and diff DTOs (default `web/src/api/generated`). These are the single source of truth for the web client's types; CI enforces they stay in sync.

## `domarinn schema <config|result>`

Print a JSON Schema to stdout — for editor completion and as a CI contract. `config` is the suite schema; `result` is the `RunResult` schema. The checked-in `domarinn.schema.json` is regenerated from `schema config` and CI fails on drift.

```sh
domarinn schema config > domarinn.schema.json
```

## `domarinn list <tests|providers|prompts> [PATH] [--json] [--generators]`

List what a suite resolves to. `--json` emits a JSON array.

`list tests` resolves inline cases, `file://` globs, and matrix expansion. It does **not** run the suite's `generator:` commands unless you pass `--generators`: a generator only produces cases by being executed, and `list` is otherwise a read-only command. Without the flag, a suite with generators gets a note on stderr saying how many were skipped; with it, the produced ids appear in the listing exactly as they will at run time, which is what makes them usable as `--filter` targets.

```sh
domarinn list providers examples/12-render-health
domarinn list tests . --json
domarinn list tests . --generators
```

## `domarinn server [--port N] [--data-dir DIR]`

Run the self-hostable results server + embedded web UI (default port `8321`, binds `0.0.0.0`; default data dir `/data`, env `DOMARINN_DATA_DIR`). See [server.md](server.md) and [self-host.md](../guides/self-host.md).

```sh
domarinn server --port 8321 --data-dir ./data
```

## `domarinn healthcheck [--port N]`

Probe **this binary's own** server health and exit `0`/non-zero accordingly. Designed for the container `HEALTHCHECK` in the distroless image, which has no shell or curl.

---

## CI usage

- **Validate on every push:** `domarinn validate` (fast, no provider calls).
- **Gate PRs on eval quality:** `domarinn run --against server:baseline` (exit `1` on regression), or use the reusable action at `.github/actions/domarinn-eval`. See [gate-in-ci.md](../guides/gate-in-ci.md).
- **Contract-test the schema:** regenerate `domarinn schema config` and fail on drift (wired in `ci.yml`).
- **Read the exit code**, not just stdout: `1` = the model regressed (block the PR), `3` = the harness broke (retry / page an operator, don't blame the PR).
