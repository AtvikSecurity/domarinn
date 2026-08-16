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
| `1` | assertion | An assertion failed, an [`expect_fail`](domarinn-yaml.md#inline-and-loaded-test-fields) case passed (`xpass` — the marker is stale and must be removed), or a run regressed against a `--against` baseline. An **expected** failure (`xfail`) never trips this. |
| `2` | config/usage | Bad config or flags, a suite that fails to load or validate (a *warning* is not one of these), a run that resolved to **zero cases**, a missing / wrong-shaped provider credential, or a `--share` **preflight refusal**: the server stated a result-schema window this CLI is outside of — or no server URL is configured at all — so the run is refused before it spends anything. |
| `2` | config/usage | A run that **formed no opinion**: every case was skipped by `runner.skip_on_empty_reason`, or every **graded** case — total minus skipped — was answered by a [`fallback:`](providers.md#falling-back-to-another-provider) provider, so the configured one answered nothing. |
| `3` | infra | Infrastructure error — a provider crashed, a grader was missing/broke, the server was unreachable, a `--cache-only` run could not answer honestly (a miss, or a case whose `latency` assertion always needs a live call), or a **`run --share` upload failed** without `--allow-share-failure`. |

Because `3` outranks `1`, a run whose assertions failed *and* whose upload failed exits `3`: the results are graded but not published, and re-running is the wrong response to either half.

The two `2`s in the second row are the same argument twice: a green gate would mean the suite ran, not that it passed. A run where every case skipped graded nothing at all; a run answered entirely by fallbacks graded a different system than the one it names. A **partial** fallback stays green on purpose — that is the feature working, and failing on it would make `fallback:` a liability rather than resilience. Use `--no-fallback` where a gate should fail on the primary directly.

---

## `domarinn run [PATH] [flags]`

Execute a suite: render prompts, call providers, evaluate assertions, report results, and persist the run under `.domarinn/runs/<id>/`.

- `PATH` — a suite file or a directory containing `domarinn.yaml` (default `.`).

| Flag | Effect |
|------|--------|
| `--tag <T>` | Only run tests with this tag (repeatable; OR within tags, AND across kinds). |
| `--filter <GLOB>` | Only run tests whose id matches this glob (repeatable). |
| `--provider <ID>` | Only run this provider (repeatable). It selects among *cells* and does not strip a chain — a selected cell may still reach its [`fallback:`](providers.md#falling-back-to-another-provider) targets, because a run that silently lost the resilience you configured would look exactly like fallback not working. The whole list is vetted, not just its intersection: any id that is unknown, [`fallback_only`](providers.md#fallback_only-a-provider-that-is-reachable-but-never-a-cell), or an embeddings provider is a usage error (exit `2`) **even beside valid ids** — a half-right list must refuse rather than silently shrink the matrix a CI job believes it measured. `DOMARINN_PROVIDER` (comma-separated) sets the same thing; the flag wins outright, and an empty or blank value counts as unset. |
| `--prompt <ID>` | Only run this prompt (repeatable). |
| `--no-cache` | Never read or write the cache. |
| `--cache-only` | Read cache only; a miss is an infrastructure error (offline CI). The credential preflight is skipped, and a case carrying a `latency` assertion is refused rather than called live — see [caching.md](../concepts/caching.md#cache-modes). |
| `--no-grader-cache` | Grader-originated requests (the LLM grader, `exec` graders, embeddings) bypass the cache; responses of the systems under test are still replayed. Use it to measure grader variance deliberately. Replaces the deprecated suite key `cache.grader: false`. |
| `--cache-dir DIR` | Where the local cache lives. Defaults to `.domarinn/cache` beside the suite, so the same suite hits the same cache from any directory. `DOMARINN_CACHE_DIR` sets the same thing; the flag wins. |
| `--no-cache-migration` | Skip looking for entries written under an older cache-key shape. domarinn probes for those on a miss so an upgrade does not discard a warm cache, and stops once it is clear there is nothing to find. |
| `--store-empty-outputs <never\|reproducible\|always>` | Which empty provider answers are written to the cache, overriding the suite's `cache.store_empty_outputs`. Defaults to `reproducible` — an empty answer is stored only when the same request would produce it again. `DOMARINN_CACHE_STORE_EMPTY_OUTPUTS` sets the same thing; the flag wins. See [caching.md](../concepts/caching.md#empty-answers). |
| `--no-fallback` | Do not let a provider hand off to its [`fallback:`](providers.md#falling-back-to-another-provider) chain. **The right posture for a gate**: fallback is resilience, and a CI job usually wants to learn that its primary provider is broken rather than have the result quietly produced by a different model. It disables chain *walking* only: a [`fallback_only`](providers.md#fallback_only-a-provider-that-is-reachable-but-never-a-cell) provider stays out of the matrix, since membership is suite configuration and not something a resilience flag should move. `DOMARINN_NO_FALLBACK=true` sets the same thing; the flag wins, and a value that is not a boolean **refuses the run** (exit `2`) naming the value, rather than being guessed at — a typo'd `DOMARINN_NO_FALLBACK=ture` must not quietly leave fallback on in the gate that set it. |
| `--repeat <N>` | Run each cell N times (variance / pass@k). |
| `-j`, `--concurrency <N>` | Max concurrent provider calls (overrides `runner.concurrency`). |
| `--format <F>` | Output format, repeatable: `table` (default), `json`, `jsonl`, `junit`, `md`. `md` is the same run-summary Markdown as `--summary-md`, on stdout. |
| `--out <FILE>` | Write the primary output to a file instead of stdout. |
| `--no-raw` | Do not persist raw provider metadata in the result document (keeps `result.json` small). The prompt and `stop_reason` are still captured. |
| `--no-progress` | Disable the live progress bar (see below). |
| `--allow-empty` | Succeed even if the run resolves to zero cases. Without it that is exit 2, because a green result over no cells is indistinguishable from a green result over every cell. Pass it for a sharded matrix where a shard legitimately has no work. |
| `--against <REF>` | Compare against a baseline. `server:baseline` uses the pin for this suite on the results server (a run *or* a branch — the reliable choice in CI); `server:branch:<name>` merges the newest server runs on `<name>` with no pin at all; `branch:<name>` does the same from the local run store; `latest` uses the newest local run *of the same suite*; a run id or a `result.json` path names one run; `none` disables the comparison (overriding a [`baseline:`](domarinn-yaml.md#baseline) suite default). A branch resolves to a *composite* — per case, the newest run on the branch wins, so a filtered newest run cannot shrink the gate's coverage. A regression sets exit code `1`; a baseline that was requested but could not be resolved sets exit code `2`. |
| `--summary-md <FILE>` | Write a Markdown summary (headline metrics table, failing cases, and any baseline comparison). Identical to what [`ci-summary`](#domarinn-ci-summary-run-flags) writes, minus the step outputs. |
| `--share` | Upload the completed run to the configured server, and record the returned URL on the stored run. **Fail-closed**: a rejected or unreachable upload logs an `ERROR` naming the run id, suite and case count, and exits `3`. Before the runner starts, `--share` also asks the server what result-schema versions it accepts; a confirmed mismatch — or no server URL configured at all — prints the remedy and exits `2` having spent nothing. See [Sharing a run](#sharing-a-run). |
| `--allow-share-failure` | Opt out of the above (requires `--share`): an upload failure becomes a `WARN` and the exit code reflects the assertions alone, and a preflight refusal becomes a warning the run proceeds past. For a fork's pull request with no server credentials, or anywhere publishing is genuinely optional. |
| `--note <TEXT>` | A short human label for this run ("trying temperature 0.3"). Stored on the run and full-text searchable on the server. Defaults to the suite's `description`. |
| `--no-provenance` | Do not record the OS username or hostname. Git, CI and version metadata are still recorded, and the run is marked redacted. |

```sh
domarinn run examples/12-render-health                 # run, print a table
domarinn run --tag safety -j 8 --format junit --out results.xml
domarinn run --against server:baseline --summary-md summary.md
domarinn run --note "retry backoff, 3rd attempt"    # label this run
```

### Sharing a run

`--share` publishes the completed run to `--server-url` / `DOMARINN_SERVER_URL` and records the returned URL on the stored run, which is where [`ci-summary`](#domarinn-ci-summary-run-flags) reads it from for the PR comment's `View run` link.

**Upload failure fails the run.** Storing the results is the point of the flag in CI, and exiting `0` having stored nothing reports a green job for work nobody can find — a misconfiguration that otherwise survives indefinitely, because the only symptom is a server that quietly stays empty. So a rejected upload or an unreachable server exits `3`. The `ERROR` names the `run_id`, suite and case count, because the run is graded and on disk: the recovery is `domarinn share <run_id>` once the server is reachable, not a re-run that pays for every provider call again.

**The schema preflight.** Before the runner starts, `--share` issues one `GET /api/v1/meta` (5s timeout) and compares the result-schema versions the server accepts against the one this CLI writes. A **confirmed** mismatch exits `2` with a message naming which side to upgrade — a window entirely below ours is a server too old to store what we write; anything else is a CLI too old to write what the server now stores. Everything short of that proceeds with a warning: unreachable, slow, `404`, unparsable, and a response that states no window are all cases where the server has said nothing to be outside of, and the upload itself is the authoritative answer. **No server URL configured at all** is the one refusal that needs no server: the upload cannot happen, so the run would be certain to fail *after* spending its provider budget — it exits `2` here instead, having spent nothing. The preflight only ever refuses; it never makes a run that would have succeeded fail later.

**Opting out.** `--allow-share-failure` (which requires `--share`) demotes both: the upload failure to a `WARN`, and the preflight refusal to a warning the run proceeds past.

### Run provenance

Every run records who and what produced it, in the engine — so a plain `domarinn run` carries it, not only a shared one:

| Field | Source |
|-------|--------|
| `origin.actor` | `DOMARINN_ACTOR`, else the CI actor (`GITHUB_ACTOR`, `GITLAB_USER_LOGIN`, …), else the OS username. In CI the CI actor wins because the OS user there is a service account that identifies nobody. |
| `origin.host` | `DOMARINN_HOST`, else the hostname. |
| `origin.version` | The domarinn build that produced the document. |
| `origin.note` | `--note`, else the suite's `description`. |
| `git` | Branch, commit and dirty state of the repo containing the suite. The **branch** comes from `DOMARINN_BRANCH`, else the CI environment (`GITHUB_HEAD_REF`/`GITHUB_REF`, `CI_COMMIT_BRANCH`, `BUILDKITE_BRANCH`, `CIRCLE_BRANCH`, `BUILD_SOURCEBRANCH`, `GIT_BRANCH`, …), else the checked-out branch. CI wins for the same reason the actor does: a runner checks out a detached HEAD, so git alone reports `HEAD` for every CI run. A tag build, a pull-request merge ref and a detached checkout all record **no** branch rather than a fake one. |
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

Parse and structurally validate a suite. **No provider calls.** Use it in pre-commit and CI to catch config errors fast. Exit `0` when the suite has no errors — it prints a one-line summary on stdout and any **warnings** on stderr; exit `2` and lists the issues otherwise.

A **warning never changes the exit code.** Three shapes warn today: a case (or `defaults`) history whose first non-`system` turn is `assistant`, or is `tool` — both only *when the suite splices history at the front of a transcript* — and a turn whose `content` is empty. All three are near-certain provider 400s and all three are legal in principle (an Anthropic assistant prefill is the first), so domarinn says so and gets out of the way. `domarinn run` reports the same warnings as `WARN` log lines and proceeds.

One transcript problem is an **error**, not a warning: a turn with neither `content` nor `tool_calls` cannot mean anything to any provider, so it exits `2` like any other malformed suite.

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
| `--raw` | With `--case`, include the raw provider metadata (schema v2 and newer). |

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

**This subcommand is the opposite default from [`run --share`](#sharing-a-run), deliberately.** `share` is best-effort unless `--strict`; `run --share` is fatal unless `--allow-share-failure`. The run this command uploads already exists on disk, so a failed upload costs a retry of the upload and nothing else — while `run --share` has just spent a suite's worth of provider calls whose whole destination was the server. `share` also does **not** preflight the server's schema window: there is no run ahead of it to protect from being wasted, and the upload's own response is the authoritative answer either way.

```sh
DOMARINN_SERVER_URL=https://evals.example domarinn share --strict
DOMARINN_SERVER_URL=https://evals.example domarinn share 01JD3V9GQ8 --strict
```

## `domarinn baseline <show|set|clear> [flags]`

Manage the server-side baseline pin for the suite named by the local `domarinn.yaml` — the pin `--against server:baseline` resolves. The same endpoints as the web UI's pin button; server from `--server-url` / `DOMARINN_SERVER_URL`, token from `DOMARINN_TOKEN`. The suite must set both `project:` and `suite:` (exit `2` otherwise — the server keys baselines on the pair).

- `show [PATH]` — print the pin: a fixed run, or a branch (auto-tracking: the newest runs on the branch merge into the comparison). An unpinned suite is stated plainly, exit `0`.
- `set <RUN> | set --branch <NAME> [--path PATH]` — pin a run (`latest`, a stored id, or a `result.json` path resolves locally to its concrete id; a bare id passes through for runs that exist only on the server) or pin a branch. A branch pin needs no run to exist yet — pinning `main` before the first upload is the natural bootstrap.
- `clear [PATH]` — remove the pin. Clearing nothing is success: the requested end state holds.

```sh
DOMARINN_SERVER_URL=https://evals.example domarinn baseline set --branch main
DOMARINN_SERVER_URL=https://evals.example domarinn baseline show
```

## `domarinn ci-summary [RUN] [flags]`

Summarize a stored run for CI: a Markdown report for a PR comment or job summary, plus the headline numbers as GitHub Actions step outputs. See [`gate-in-ci.md`](../guides/gate-in-ci.md#the-ci-summary-command).

| Flag | Meaning |
|---|---|
| `RUN` | Run to summarize — a run id, `latest` (default), a `result.json`, or a run directory. |
| `--against <REF>` | Append a baseline comparison; same references as `run --against`, including the branch forms and `none`. With no flag, a [`baseline:`](domarinn-yaml.md#baseline) key in the summarized run's config supplies the same default the gate used. `ci-summary` is a reporter, not a gate, so an unresolvable baseline warns and is skipped rather than failing. |
| `--out <FILE>` | Write the Markdown to a file instead of stdout. |
| `--github-output <FILE>` | Append `key=value` step outputs here. Defaults to `$GITHUB_OUTPUT`, so on a runner no flag is needed. |

**It is a reporter, not a gate** — it exits `0` for a failing run, because the verdict belongs to `run`'s [exit code](#exit-codes). It exits `2` only when the run reference cannot be resolved.

```sh
domarinn ci-summary                              # latest run, Markdown on stdout
domarinn ci-summary --against server:baseline --out summary.md
```

## `domarinn cache <stats|path|gc|clear>`

Manage the **local** content-addressed cache. All of these take a suite path (default `.`) and the same `--cache-dir` a run does, so they inspect the directory that run would actually use.

- `cache stats` — entry count and total size.
- `cache path` — print the cache directory (`.domarinn/cache` beside the suite).
- `cache gc [--older-than <30d|12h|45m|90s>] [--newer-than <D>] [--empty-reason <R>]…` — remove entries matching an age window, an empty reason, or both. See below.
- `cache clear` — remove all entries.
- `cache ls [--kind K] [--model M] [--limit N] [--json]` — list entries: key, kind, model, size, and where the request went.
- `cache show <KEY> [--raw] [--json]` — one entry in full: the request it answers, the response it returned, tokens and cost. `--raw` adds the provider's raw metadata, which is withheld by default because it is the largest part of an entry and the least often wanted.

`ls` and `show` answer the question a browser cannot reach: *why did this case replay a stale answer?* That question is asked at a terminal, in a repo, with a warm `.domarinn/cache` and no server running. They are also what makes the rebuilt-program warning actionable — domarinn tells you it is replaying answers produced by a different build of a provider's program, and `cache show` is how you look at one.

`--json` emits one object per line, so it composes with `jq`, `grep` and `head`. Both commands read the same tiers a run does, including the read-only legacy tier: an `ls` that omitted a tier a run can still hit would be an `ls` that lies.

`show` distinguishes its failures: a malformed key is a usage error (exit `2`), a well-formed key that is simply not present is exit `3` — the caller asked a sensible question and the answer is no.

### `cache gc` predicates

| Flag | Effect |
|------|--------|
| `--older-than <D>` | Only entries last modified before `now - D`. |
| `--newer-than <D>` | Keep the purge away from entries younger than `D` — the recent end of a window. **Requires `--older-than`.** |
| `--empty-reason <R>` | Only entries whose recorded [empty reason](../concepts/caching.md#empty-answers) is `R`. Repeatable; the matches are OR'd. |

`gc` prints `removed N of M`, so a targeted eviction says how much of the store it left alone.

Three rules, each of which exists because the obvious alternative deletes something nobody asked for:

- **At least one bound is required — any one of the three.** A bare `domarinn cache gc` is a usage error (exit `2`), because the obvious reading of "gc" is "tidy up a bit" and the command that removes everything should be the one that says so. The error names `cache clear`.
- **`--newer-than` needs `--older-than`.** Alone it would read "delete everything since", which is a whole-store wipe in the vocabulary of housekeeping. Requiring the other end makes that reading unconstructible rather than merely discouraged.
- **No environment variables here.** `DOMARINN_CACHE_STORE_EMPTY_OUTPUTS` and `DOMARINN_NO_FALLBACK` exist because they are standing *policy*. A `gc` predicate is a one-shot destructive operation, and an environment that silently widens one is how an operator deletes a corpus they never named. That is the line, not "environment variables are bad": run-scoping selection like [`DOMARINN_PROVIDER`](#domarinn-run-path-flags) is allowed on the run side — the flag still wins, and the worst an unnoticed one does is run *fewer* cells, which the next invocation undoes.

Reach for `--empty-reason` when a bad draw has been frozen into the store: `cache gc --empty-reason refusal --older-than 0s` removes those entries and nothing else, where an age-only `gc` would have to throw away the warm cache around them. It is the local half of the [targeted eviction](../concepts/caching.md#removing-entries-you-already-have) story; the server half is `DELETE /api/v1/cache/entries/{key}` and `POST /cache/prune`.

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

## `domarinn migrate-db [--data-dir DIR] [--database-url URL]`

Copy an existing SQLite data dir into an **empty** Postgres database, so a deployment can switch storage backends without losing its history. One-shot and offline: **stop the server first** — SQLite is a single writer, and the migration must be the only thing holding the database open.

| Flag | Effect |
|------|--------|
| `--data-dir <DIR>` | The SQLite data dir to migrate from (also `DOMARINN_DATA_DIR`). |
| `--database-url <URL>` | The target Postgres connection URL (also `DOMARINN_DATABASE_URL`). |

What it does, in order: brings the SQLite source up to the **latest schema** (the same migrations the server runs at startup), **refuses a non-empty target** — an accidental URL must not merge two histories — copies everything (runs, users, sessions, API keys, baselines, and the shared cache), **verifies per-table row counts**, and prints a summary. The SQLite files are not modified beyond that schema upgrade, so they remain a rollback: point the server back at the data dir and it runs as before.

Afterwards, start the server with `DOMARINN_DATABASE_URL` set. See [server.md](server.md#storage) for what the Postgres backend changes and [self-host.md](../guides/self-host.md#postgres) for the hosting walkthrough.

```sh
domarinn migrate-db --data-dir /data --database-url "postgres://domarinn:secret@db.example.com:5432/domarinn"
```

## `domarinn healthcheck [--port N]`

Probe **this binary's own** server health and exit `0`/non-zero accordingly. Designed for the container `HEALTHCHECK` in the distroless image, which has no shell or curl.

---

## CI usage

- **Validate on every push:** `domarinn validate` (fast, no provider calls).
- **Gate PRs on eval quality:** `domarinn run --against server:baseline` (exit `1` on regression), or use the reusable action at `.github/actions/domarinn-eval`. Pin a branch once (`domarinn baseline set --branch main`) and the baseline auto-tracks it; `--against server:branch:main` needs no pin at all. See [gate-in-ci.md](../guides/gate-in-ci.md).
- **Contract-test the schema:** regenerate `domarinn schema config` and fail on drift (wired in `ci.yml`).
- **Read the exit code**, not just stdout: `1` = the model regressed (block the PR), `3` = the harness broke (retry / page an operator, don't blame the PR).
