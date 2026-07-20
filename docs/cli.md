# measurellm CLI reference

One binary, `measurellm`. It is the eval engine, the results server, and its own
container healthcheck. Global flags apply to every subcommand.

```
measurellm [-v|-vv] <command> [args]
```

| Global flag | Effect |
|-------------|--------|
| `-v`, `-vv` | Increase log verbosity (`warn` -> `info` -> `debug`). Logs go to **stderr**. |
| `--version` | Print the version. |
| `--help`    | Print help for the binary or a subcommand. |

Logging respects `RUST_LOG` if set (e.g. `RUST_LOG=measurellm=debug`).

## Exit codes

The exit code is a **contract for CI** — it distinguishes "the model got worse"
from "the harness broke". `3` (infra) wins over `1` (assertion) when both occur.

| Code | Name          | Meaning |
|------|---------------|---------|
| `0`  | OK            | Everything passed. |
| `1`  | assertion     | Assertion failure or a regression vs a baseline. |
| `2`  | config/usage  | Bad config, bad flags, or a suite that fails to load/validate. |
| `3`  | infra         | Infrastructure error — a provider crashed, the server was unreachable, an internal error. |

## Implementation status

Wired today: **`validate`**, **`schema`**, **`list`**, **`server`**,
**`healthcheck`**. Landing in later phases: **`run`**, **`share`**, **`cache`**,
**`diff`**. The full surface is documented here as the stable contract; planned
commands are marked _(planned)_.

---

## `measurellm validate [PATH]`

Parse and structurally validate a suite. **No provider calls.** Use it in
pre-commit and CI to catch config errors fast.

- `PATH` — a suite file or a directory containing `measurellm.yaml` (default
  `.`).

Exit `0` when valid (prints a one-line summary of providers/prompts/test
sources); exit `2` and lists issues otherwise.

```sh
measurellm validate examples/render-health
```

## `measurellm run [PATH] [flags]` _(planned)_

Execute a suite: render prompts, call providers, run assertions, print results.

- `PATH` — suite file or directory (default `.`).
- `--against <baseline>` — diff this run against a baseline (a run id, `latest`,
  or a git ref) and report regressions.
- `--threshold <0.0-1.0>` — fail if the overall pass rate drops below this.
- `--server-url <url>` / `MEASURELLM_SERVER_URL` — upload the run and get a
  share link.
- `--share` — upload the completed run to the configured server.
- `--summary-md <file>` — write a Markdown summary (used by the CI action).

Exit `0` all pass; `1` on assertion failure/regression or a threshold miss; `2`
config/usage; `3` infra.

## `measurellm schema <config|result>`

Print a JSON Schema to stdout — for editor completion and as a CI contract.

```sh
measurellm schema config > measurellm.schema.json
```

`config` is the suite schema; `result` is the run-result (`RunResult`) schema.
The checked-in `measurellm.schema.json` is regenerated from `schema config` and
CI fails if it drifts (see `.github/workflows/ci.yml` and `just schema`).

## `measurellm list <tests|providers|prompts> [PATH] [--json]`

List what a suite resolves to.

- `tests` | `providers` | `prompts` — what to list.
- `PATH` — suite file or directory (default `.`).
- `--json` — emit a JSON array instead of a table/lines.

```sh
measurellm list providers examples/render-health
measurellm list tests . --json
```

## `measurellm server [--port N] [--data-dir DIR]`

Run the self-hostable results server + embedded web UI.

- `--port` — listen port (default `8321`; binds `0.0.0.0`).
- `--data-dir` — state directory (default `/data`; env `MEASURELLM_DATA_DIR`).

Health is served at `/health` and `/api/v1/health`. See
[deploy.md](./deploy.md) for `MEASURELLM_TOKENS`, `MEASURELLM_PUBLIC_URL`, and
hosting notes. Runs until Ctrl-C (graceful shutdown).

```sh
measurellm server --port 8321 --data-dir ./data
```

## `measurellm share <RUN> [flags]` _(planned)_

Upload a completed run's results to a server and print a share link.

- `RUN` — path to a stored run (or the last run).
- `--server-url <url>` / `MEASURELLM_SERVER_URL` — target server.
- `--token <secret>` — bearer token when the server requires auth.

Exit `0` on success; `3` if the server is unreachable.

## `measurellm cache <subcommand>` _(planned)_

Inspect and manage the content-addressed cache.

- `cache stats` — size and hit-rate summary.
- `cache clear` — drop cached entries.
- `cache path` — print the cache location.

## `measurellm diff <A> <B>` _(planned)_

Diff two runs and report regressions/improvements per test. Exit `1` when `B`
regressed against `A`; `0` otherwise. Underpins `run --against`.

## `measurellm healthcheck [--port N]`

Probe **this binary's own** server health and exit `0`/non-zero accordingly.
Designed for container `HEALTHCHECK` in the distroless image, which has no shell
or curl — the binary is its own probe.

- `--port` — port to probe (default `8321`).

```sh
measurellm healthcheck            # exit 0 if the local server is healthy
```

---

## CI usage

- **Validate on every push:** `measurellm validate` (fast, no provider calls).
- **Gate PRs on eval quality:** `measurellm run --against latest
  --threshold 0.95`, or use the reusable action at
  `.github/actions/measurellm-eval`.
- **Contract-test the schema:** regenerate `measurellm schema config` and fail
  on drift (already wired in `ci.yml`).
- **Read the exit code**, not just stdout: `1` = the model regressed (block the
  PR), `3` = the harness broke (retry / page an operator, don't blame the PR).
