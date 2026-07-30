# CI integration

domarinn is built to gate pull requests: it runs an eval suite, compares against a baseline, writes machine-readable reports, and — crucially — returns an **exit code that distinguishes "the model regressed" from "the harness broke".** This page covers the exit-code contract, the reusable GitHub Action, a worked walkthrough of gating on a regression, gating a PR by hand, and uploading CI runs to a shared server.

- [The exit-code contract](#the-exit-code-contract)
- [The reusable action](#the-reusable-action)
- [The `ci-summary` command](#the-ci-summary-command)
- [Gate a pull request on regressions](#gate-a-pull-request-on-regressions)
- [Gating a PR by hand](#gating-a-pr-by-hand)
- [Uploading CI runs to a shared server](#uploading-ci-runs-to-a-shared-server)

---

## The exit-code contract

Every domarinn run exits with a code your CI can branch on. **`3` (infra) wins over `1` (assertion)** when both happen in one run — a broken harness must never masquerade as a passing eval, nor should it be blamed on the PR author.

| Code | Name         | Meaning | CI reaction |
|------|--------------|---------|-------------|
| `0`  | OK           | Everything passed. | Merge. |
| `1`  | assertion    | An assertion failed, or a run regressed vs its baseline. | Block the PR (the model got worse). |
| `2`  | config/usage | Bad config, bad flags, or a suite that won't load. | Fix the suite/workflow. |
| `3`  | infra        | A provider crashed, the server was unreachable, or an internal error. | Retry / page an operator — **not** the PR's fault. |

This is the same contract the CLI documents in [`cli.md`](../reference/cli.md#exit-codes).

---

## The reusable action

A composite action lives at [`.github/actions/domarinn-eval/action.yml`](https://github.com/AtvikSecurity/domarinn/blob/main/.github/actions/domarinn-eval/action.yml). It resolves a `domarinn` binary, runs your suite, uploads the JUnit report + Markdown summary, posts (or updates) a single PR comment, and gates the job on the CLI's exit code.

### Minimal workflow

```yaml
name: eval
on: pull_request

permissions:
  contents: read
  pull-requests: write   # required for the PR comment

jobs:
  eval:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: AtvikSecurity/domarinn/.github/actions/domarinn-eval@0.1.0
        with:
          config: eval/domarinn.yaml
          against: server:baseline
```

### Inputs

| Input                | Default             | Purpose |
|----------------------|---------------------|---------|
| `config`             | `domarinn.yaml`   | Suite file, or a directory containing `domarinn.yaml`. |
| `binary-path`        | `""`                | Path to a prebuilt `domarinn`. When set, binary resolution stops here (no download, no build). |
| `version`            | `latest`            | Release tag to download the binary from (e.g. `0.1.0`), or `latest`. Used only when `binary-path` is empty. |
| `server-url`         | `""`                | Results server base URL. When set, the run is uploaded with `--share` (exported as `DOMARINN_SERVER_URL`). |
| `token`              | `""`                | Bearer token for the results server (exported as `DOMARINN_TOKEN`). Pass a **secret**, never a literal. |
| `against`            | `""`                | Baseline to diff against. Use `server:baseline` in CI (the suite's pinned baseline on the server); `latest` reads the local run store, which a fresh checkout does not have. Also accepts a run id or a `result.json` path. Empty disables the diff. A regression makes the CLI exit 1; a baseline that cannot be resolved makes it exit 2. |
| `fail-on-regression` | `"true"`            | If `true`, exit 1 (assertion/regression) fails the check. If `false`, exit 1 is a warning only. Exit 2 and 3 **always** fail. |
| `comment`            | `"true"`            | Post/update the summary comment on the PR. |
| `artifact-name`      | `"domarinn-results"` | Name of the uploaded artifact holding `results.xml` + `summary.md`. |
| `allow-empty`        | `"false"`           | If `true`, succeed when the run resolves to zero test cases (becomes `--allow-empty`). Off by default: a green result over no cells is indistinguishable from a green result over every cell. Turn it on for a sharded matrix where a shard legitimately has no work. |
| `cache-dir`          | `""`                | Directory for the local cache — typically the path `actions/cache` restored (becomes `--cache-dir`). Empty uses `.domarinn/cache` beside the suite, which a cache step rarely saves. See [Shared cache for CI](#uploading-ci-runs-to-a-shared-server). |

### Outputs

Every count below comes from [`domarinn ci-summary`](#the-ci-summary-command), which writes them straight to `$GITHUB_OUTPUT` — the action does not parse a report to produce them.

| Output           | Meaning |
|------------------|---------|
| `exit-code`      | The CLI's raw exit code (`0`/`1`/`2`/`3`). |
| `passed`         | Number of cases that passed. |
| `failed`         | Number of cases that failed **or** errored. |
| `errored`        | Number of cases that errored (a subset of `failed`). |
| `total`          | Total number of cases. |
| `pass-rate`      | Percentage of cases that passed, one decimal (e.g. `91.7`). |
| `cost-usd` | Cost of the work this run represents, in USD (empty when unpriced). |
| `cache-savings-usd` | What the cached cases would have cost. Actual spend is `cost-usd` minus this. |
| `grader-cost-usd` | What the graders cost, in USD. **Not** part of `cost-usd`, which is what the systems under test cost. |
| `cache-read-tokens` | Input tokens served from a provider-side prompt cache. |
| `cache-write-tokens` | Input tokens written into that cache. |
| `cache-hit-rate` | Percentage of cases served from the cache, one decimal. |
| `regressed`      | Number of tests newly regressed vs the baseline (`0` without `against`). |
| `run-url`        | URL of the uploaded run on the results server (empty without `server-url`). |
| `summary-path`   | Path to the Markdown summary (`summary.md`). |
| `results-path`   | Path to the JUnit report (`results.xml`). |

`failed` keeps its original meaning — failures *and* errors — so an existing workflow that gates on it is unaffected. `errored` is the new way to tell the two apart without re-reading the report.

### What it does, step by step

1. **Resolve the binary.** Provided `binary-path` → download `domarinn-<target>` from the repo's GitHub Releases (`version` or `latest`, arch auto-detected) → **fallback** to building from source with `cargo` (`cargo install --path crates/domarinn-cli` if this repo is checked out, else `cargo install --git …`). The cargo fallback requires a Rust toolchain on the runner — add `dtolnay/rust-toolchain` before this action if you rely on it.
2. **Run the suite:** `domarinn run <config> --format junit --out results.xml`, appending `--against <against>`, `--cache-dir <cache-dir>`, `--allow-empty` and `--share` when those inputs are set. It captures the exit code without aborting so the later steps still run.
3. **Summarize** — `domarinn ci-summary latest --out summary.md` writes the Markdown and appends the headline numbers to `$GITHUB_OUTPUT` itself. If the suite never produced a run (a config error, say), the step warns and writes a placeholder summary rather than leaving the PR comment blank.
4. **Upload** `results.xml` + `summary.md` as an artifact (**always**, even on failure), and append the summary to the job's step summary.
5. **Comment on the PR** — creates or updates one comment (matched by a hidden `<!-- domarinn-eval -->` marker) so repeated pushes don't spam the thread. Skipped unless `comment: true` and the event is a `pull_request`.
6. **Gate** on the exit code: `0` passes; `1` fails **only when `fail-on-regression` is true** (otherwise a warning); `2` and `3` always fail.

### Advanced usage

```yaml
- uses: AtvikSecurity/domarinn/.github/actions/domarinn-eval@0.1.0
  with:
    config: eval/
    against: server:baseline
    fail-on-regression: "true"
    server-url: https://domarinn.example.com   # enables --share
    token: ${{ secrets.DOMARINN_TOKEN }}        # write-scoped token
```

---

## The `ci-summary` command

`domarinn ci-summary` turns a stored run into the two things a workflow wants: a Markdown report to post, and `key=value` step outputs to branch on. The [action](#the-reusable-action) calls it for you; reach for it directly when you hand-roll a pipeline, or on a forge that is not GitHub.

```sh
domarinn run eval/ --format junit --out results.xml   # exit code gates the PR
domarinn ci-summary latest --against server:baseline --out summary.md
```

It reads a **persisted** run rather than taking one over a pipe, so it can also re-summarize an older run by id — and so a summary step cannot change what the run already decided.

### What it emits

The Markdown is a headline metrics table, then either a baseline comparison (with `--against`) or this run's failing cases, then any links:

```markdown
### domarinn run — content-safety

| metric | value |
|---|---|
| Result | ❌ 1 passed, 2 failed |
| Pass rate | 33.3% (95% CI 6.1–79.2%, n=3) |
| Cache | 2/3 cases from cache (66.7%) |
| Duration | 4.1s |

**Failing:**

| test | provider | score | why |
|---|---|---|---|
| refuses-pii | gpt-4.1 | 0.00 | contains: output does not contain "I can't help" |

[View run](https://domarinn.example.com/runs/01JD3V9GQ8) · [CI run](https://github.com/acme/widgets/actions/runs/42)
```

A few deliberate choices:

- **Rows that carry no information are omitted.** No `Cost` row when nothing was billed, no `Retries` row when nothing retried. What is printed is what happened.
- **`Cache` counts *cases*, not lookups.** `cache_misses` counts every case not served from the cache, so under `--no-cache` it equals the case count — there is no lookup total to express a "hit rate" against.
- **The failing table is capped at 10 rows**, then `…and N more`. The run URL and the JUnit artifact hold the full list; GitHub truncates a huge comment anyway.
- **Table cells are escaped.** An assertion reason quoting model output that contains a `|` would otherwise shred the table.
- **Both links degrade.** `View run` needs a run uploaded with `--share`; `CI run` comes from the run's recorded CI metadata, falling back to the ambient workflow environment. Neither present means no links line at all.

### Step outputs

With `$GITHUB_OUTPUT` set (automatic on a runner) or `--github-output <file>`, it **appends** — a job's steps share one file, so truncating would discard earlier steps' outputs:

```
passed=1
failed=2
errored=0
failed-or-errored=2
total=3
pass-rate=33.3
cache-hits=2
cache-misses=1
cost-usd=0.041200
cache-savings-usd=0.018700
grader-cost-usd=0.009400
cache-read-tokens=104000
cache-write-tokens=8200
cache-hit-rate=66.7
regressed=0
run-url=
ci-run-url=https://github.com/acme/widgets/actions/runs/42
```

Keys are written even when empty, so a workflow referencing one always gets a defined value.

### It never gates

`ci-summary` exits `0` for a failing run. The verdict is `run`'s [exit code](#the-exit-code-contract) and only that — a summary step that could also fail a job would gate the same failure twice and obscure which one spoke. The single exception is an unresolvable run reference, which exits `2` as a usage error.

---

## Gate a pull request on regressions

**The problem.** A behavioural suite is never 100% green. Gating on an absolute pass rate means either setting the bar so low it catches nothing, or blocking every merge. What you actually want to block is a change that made things **worse** — which is exactly what the action and the exit-code contract above are for. Here is the whole thing wired together.

### 1. Store runs somewhere CI can reach

Regression gating needs a baseline that survives a fresh checkout. Run the [results server](../reference/server.md) and point runs at it:

```console
$ export DOMARINN_SERVER_URL=https://evals.example.com
$ export DOMARINN_TOKEN=...
$ domarinn run eval/behavioral.yaml --share
```

Then pin a known-good run as the baseline for that project/suite in the web UI.

### 2. Gate against it

```yaml
- name: Behavioral evaluation
  uses: AtvikSecurity/domarinn/.github/actions/domarinn-eval@<sha>  # keep in lockstep with the version below
  with:
    config: eval/behavioral.yaml
    version: ${{ env.DOMARINN_VERSION }}
    server-url: https://evals.example.com
    token: ${{ secrets.DOMARINN_TOKEN }}
    against: server:baseline
    fail-on-regression: "true"
```

```yaml
--8<-- "examples/24-baselines-and-diff/domarinn.yaml"
```

### 3. Know exactly what the gate does and does not catch

/// danger | Three ways this gate silently passes

**`--against latest` in CI.** It resolves through a cwd-relative `.domarinn/runs/latest`, which a fresh checkout does not have. It finds nothing, warns, and exits `0` on a real regression. Use `server:baseline`.

**No baseline pinned.** Also a warning, not a failure. The gate is only ever as good as what is actually pinned — and a stale or one-case baseline compares almost nothing while reading as a pass. Re-pin it after any deliberate improvement, and check what it contains.

**A version mismatch.** The action shells out to `domarinn ci-summary`. Against an older binary that subcommand does not exist, and the step degrades to a stub comment on a green run. Pin the action and `DOMARINN_VERSION` as a pair.

///

Also note baselines are keyed **per provider id**. Renaming a provider — or changing the model inside its `command` — starts its history over, silently.

### 4. Remember which code wins

Recall [the exit-code contract](#the-exit-code-contract): `3` beats `1` when both occur in one run, so a run that both regressed *and* broke reports as broken — the honest answer, since the regression may be an artefact of the breakage.

### 5. Report it where people look

```console
$ domarinn ci-summary latest --out summary.md --github-output "$GITHUB_OUTPUT"
```

That's the same [`ci-summary`](#the-ci-summary-command) command from above, run against this baseline-gated result — a reporter, never a gate, so a reporting failure never blocks a merge and never fakes one.

---

## Gating a PR by hand

The minimal gate is a single `run` invocation whose exit code you read:

```sh
domarinn run \
  --against server:baseline \
  --format junit --out results.xml \
  --summary-md summary.md
# exit 0 pass · 1 fail/regression · 2 config/unresolvable baseline · 3 infra
```

- `--against server:baseline` diffs this run against the baseline **pinned for this suite on the results server** and turns a **regression into exit 1**.

  Use this one in CI. `--against latest` reads the local `.domarinn/runs` store, which a fresh checkout does not have — so in CI it finds no baseline, compares nothing, and the job passes. It also needs `project:` and `suite:` set in your config, since the server pins one baseline per `(project, suite)`.

  Pin a baseline from the run's page in the web UI, or with `PUT /api/v1/projects/{project}/suites/{suite}/baseline`.

- `--against latest` is the **local** equivalent: the newest run *of the same suite* in `.domarinn/runs`. You can also pass a run id or a `result.json` path.

- A baseline that was requested but cannot be resolved is **exit 2**, not a warning. A gate that silently skips its comparison is not a gate. Only a genuine absence — a suite's first run, or a suite with no baseline pinned yet — is treated as "nothing to compare" and lets the run through.
- `--format junit --out results.xml` writes a JUnit report your CI can render as test results.
- `--summary-md summary.md` writes a Markdown summary suitable for a PR comment or a job step summary — the same document [`ci-summary`](#the-ci-summary-command) produces, which you can also run as a separate step to get step outputs alongside it.

Then gate on the code:

```sh
domarinn run --against server:baseline --format junit --out results.xml --summary-md summary.md
code=$?
case "$code" in
  0) echo "all passed" ;;
  1) echo "::error::eval regressed"; exit 1 ;;
  2) echo "::error::bad config";     exit 2 ;;
  3) echo "::error::infra failure";  exit 3 ;;  # retry, don't blame the PR
esac
```

The [reusable action](#the-reusable-action) above does exactly this, plus the report upload and PR comment, so you usually don't hand-roll it.

### Stable output in CI

CI logs are deterministic: domarinn detects that its output is captured (not a terminal) and drops everything cosmetic. The **live progress bar is suppressed** on a non-TTY stderr (so no carriage returns or redraws clutter the log), and **human output is never colored** without a terminal. `stdout` — your `json`, `jsonl`, or `junit` report — is byte-for-byte identical whether or not a terminal is attached. If you ever need to force it, `NO_COLOR=1`, `--color never`, and `--no-progress` all guarantee plain, stable output regardless of environment.

---

## Uploading CI runs to a shared server

Point CI at a shared [server](../reference/server.md) so every eval is browsable and each PR gets a durable link, and so runs can share a cache of every request they make — provider calls, judge verdicts, embeddings.

- **`server-url` / `DOMARINN_SERVER_URL`** — the server base URL. Setting it makes the run upload with **`--share`**.
- **`DOMARINN_TOKEN`** (the action's `token` input) — a bearer token sent on upload. Unless the server is explicitly in `open` mode, this needs **`write`** scope (a static `write:` token or an `domarinn_` API key). Always pass it from a secret.
- **`--share`** uploads the completed run and prints `View run: <url>`; the URL uses the server's `DOMARINN_PUBLIC_URL`.

On GitHub Actions the CLI automatically enriches the uploaded run with git (branch, commit, dirty flag) and CI (provider + run URL) metadata, so shared runs are traceable back to the workflow.

**Shared cache for CI.** Multiple CI jobs can share every request domarinn makes — provider responses, judge verdicts, embeddings — through the server's content-addressed cache (`/api/v1/cache/*`), which cuts cost and time on reruns. The client side is `cache.backend: layered` plus `DOMARINN_SERVER_URL`, documented in [`caching.md`](../concepts/caching.md#backends).

A key is a hash of the request and nothing else, so a fresh checkout on a fresh runner reuses whatever another job wrote. Two things are worth setting up deliberately:

- **If the job builds its `exec` provider from source**, pin `cache_salt` to the commit SHA. The key hashes what the provider *sends*, not the compiled bytes — which is exactly why two runners can share entries at all, since Rust builds are not byte-reproducible — so nothing else tells domarinn that a rebuild happened. A run warns when it replays answers from a different build.

  ```yaml
  providers:
    - id: agent
      type: exec
      command: ["./target/release/agent"]
      cache_salt: "${env:GITHUB_SHA}"
  ```

- **If the job restores a cache directory**, hand it to the action as `cache-dir`; it becomes `--cache-dir` on the run. Leave it out and the cache lands at `.domarinn/cache` beside the suite, which is rarely the path the cache step saves.

  ```yaml
  - uses: actions/cache@v4
    with:
      path: .domarinn-cache
      key: domarinn-cache-${{ github.run_id }}
      restore-keys: domarinn-cache-

  - uses: AtvikSecurity/domarinn/.github/actions/domarinn-eval@0.1.0
    with:
      config: eval/domarinn.yaml
      cache-dir: .domarinn-cache
  ```

  The path resolves against the workspace — the same base `actions/cache` uses. A run-unique `key` with a shared `restore-keys` prefix is what lets the cache grow: a key that hits exactly is never saved again, so every case added after the first run would miss forever. Driving the CLI yourself rather than through the action, `--cache-dir` and `DOMARINN_CACHE_DIR` set the same thing.

The first CI run after upgrading to 0.5 re-files what a 0.4-era cache holds under the new keys, automatically and once — see [Upgrading to 0.5](../concepts/caching.md#upgrading-to-05). A job that pins `--cache-only` should run once in the default read-write mode first, or `similar` assertions will hard-error: their 0.4 entries are the one shape that cannot be adopted.

---

## See also

- [Example 24](../examples/caching-and-statistics.md#example-24--baselines-and-diff) — the baseline-diff suite used in the walkthrough above.
- [Statistics](../concepts/statistics.md) — McNemar significance on a paired diff.
