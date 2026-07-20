# CI integration

measurellm is built to gate pull requests: it runs an eval suite, compares
against a baseline, writes machine-readable reports, and — crucially — returns an
**exit code that distinguishes "the model regressed" from "the harness broke".**
This page covers gating a PR, the reusable GitHub Action, this repo's own CI/CD,
and uploading CI runs to a shared server.

- [The exit-code contract](#the-exit-code-contract)
- [Gating a PR by hand](#gating-a-pr-by-hand)
- [The reusable action](#the-reusable-action)
- [Uploading CI runs to a shared server](#uploading-ci-runs-to-a-shared-server)
- [This repo's CI (`ci.yml`)](#this-repos-ci-ciyml)
- [Releases (`release.yml`)](#releases-releaseyml)

---

## The exit-code contract

Every measurellm run exits with a code your CI can branch on. **`3` (infra) wins
over `1` (assertion)** when both happen in one run — a broken harness must never
masquerade as a passing eval, nor should it be blamed on the PR author.

| Code | Name         | Meaning | CI reaction |
|------|--------------|---------|-------------|
| `0`  | OK           | Everything passed. | Merge. |
| `1`  | assertion    | An assertion failed, or a run regressed vs its baseline. | Block the PR (the model got worse). |
| `2`  | config/usage | Bad config, bad flags, or a suite that won't load. | Fix the suite/workflow. |
| `3`  | infra        | A provider crashed, the server was unreachable, or an internal error. | Retry / page an operator — **not** the PR's fault. |

This is the same contract the CLI documents in [`./cli.md`](./cli.md#exit-codes).

---

## Gating a PR by hand

The minimal gate is a single `run` invocation whose exit code you read:

```sh
measurellm run \
  --against latest \
  --format junit --out results.xml \
  --summary-md summary.md
# exit 0 pass · 1 fail/regression · 2 config · 3 infra
```

- `--against latest` diffs this run against the latest baseline and turns a
  **regression into exit 1**. You can also pass a run id or a `result.json` path.
- `--format junit --out results.xml` writes a JUnit report your CI can render as
  test results.
- `--summary-md summary.md` writes a Markdown summary suitable for a PR comment
  or a job step summary.

Then gate on the code:

```sh
measurellm run --against latest --format junit --out results.xml --summary-md summary.md
code=$?
case "$code" in
  0) echo "all passed" ;;
  1) echo "::error::eval regressed"; exit 1 ;;
  2) echo "::error::bad config";     exit 2 ;;
  3) echo "::error::infra failure";  exit 3 ;;  # retry, don't blame the PR
esac
```

The [reusable action](#the-reusable-action) below does exactly this, plus the
report upload and PR comment, so you usually don't hand-roll it.

---

## The reusable action

A composite action lives at
[`.github/actions/measurellm-eval/action.yml`](../.github/actions/measurellm-eval/action.yml).
It resolves a `measurellm` binary, runs your suite, uploads the JUnit report +
Markdown summary, posts (or updates) a single PR comment, and gates the job on
the CLI's exit code.

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
      - uses: perfectra1n/measurellm/.github/actions/measurellm-eval@v1
        with:
          config: eval/measurellm.yaml
          against: latest
```

### Inputs

| Input                | Default             | Purpose |
|----------------------|---------------------|---------|
| `config`             | `measurellm.yaml`   | Suite file, or a directory containing `measurellm.yaml`. |
| `binary-path`        | `""`                | Path to a prebuilt `measurellm`. When set, binary resolution stops here (no download, no build). |
| `version`            | `latest`            | Release tag to download the binary from (e.g. `v0.1.0`), or `latest`. Used only when `binary-path` is empty. |
| `server-url`         | `""`                | Results server base URL. When set, the run is uploaded with `--share` (exported as `MEASURELLM_SERVER_URL`). |
| `token`              | `""`                | Bearer token for the results server (exported as `MEASURELLM_TOKEN`). Pass a **secret**, never a literal. |
| `against`            | `""`                | Baseline to diff against (`latest`, a run id, or a `result.json` path). Empty disables the diff. A regression makes the CLI exit 1. |
| `fail-on-regression` | `"true"`            | If `true`, exit 1 (assertion/regression) fails the check. If `false`, exit 1 is a warning only. Exit 2 and 3 **always** fail. |
| `comment`            | `"true"`            | Post/update the summary comment on the PR. |
| `artifact-name`      | `"measurellm-results"` | Name of the uploaded artifact holding `results.xml` + `summary.md`. |

### Outputs

| Output         | Meaning |
|----------------|---------|
| `exit-code`    | The CLI's raw exit code (`0`/`1`/`2`/`3`). |
| `passed`       | Number of cases that passed. |
| `failed`       | Number of cases that failed or errored. |
| `regressed`    | Number of tests newly regressed vs the baseline (`0` without `against`). |
| `summary-path` | Path to the Markdown summary (`summary.md`). |
| `results-path` | Path to the JUnit report (`results.xml`). |

### What it does, step by step

1. **Resolve the binary.** Provided `binary-path` → download
   `measurellm-<target>` from the repo's GitHub Releases (`version` or `latest`,
   arch auto-detected) → **fallback** to building from source with `cargo`
   (`cargo install --path crates/measurellm-cli` if this repo is checked out,
   else `cargo install --git …`). The cargo fallback requires a Rust toolchain on
   the runner — add `dtolnay/rust-toolchain` before this action if you rely on it.
2. **Run the suite:**
   `measurellm run <config> --format junit --out results.xml --summary-md
   summary.md`, appending `--against <against>` and `--share` when those inputs
   are set. It captures the exit code without aborting so the later steps still
   run.
3. **Parse headline numbers** — `tests`/`failures`/`errors` from the JUnit
   `<testsuite>` element, and `regressed` from the summary's `| Newly failing |
   N |` row.
4. **Upload** `results.xml` + `summary.md` as an artifact (**always**, even on
   failure), and append the summary to the job's step summary.
5. **Comment on the PR** — creates or updates one comment (matched by a hidden
   `<!-- measurellm-eval -->` marker) so repeated pushes don't spam the thread.
   Skipped unless `comment: true` and the event is a `pull_request`.
6. **Gate** on the exit code: `0` passes; `1` fails **only when
   `fail-on-regression` is true** (otherwise a warning); `2` and `3` always fail.

### Advanced usage

```yaml
- uses: perfectra1n/measurellm/.github/actions/measurellm-eval@v1
  with:
    config: eval/
    against: latest
    fail-on-regression: "true"
    server-url: https://measurellm.example.com   # enables --share
    token: ${{ secrets.MEASURELLM_TOKEN }}        # write-scoped token
```

---

## Uploading CI runs to a shared server

Point CI at a shared [server](./server.md) so every eval is browsable and each PR
gets a durable link, and so runs can share a provider cache.

- **`server-url` / `MEASURELLM_SERVER_URL`** — the server base URL. Setting it
  makes the run upload with **`--share`**.
- **`MEASURELLM_TOKEN`** (the action's `token` input) — a bearer token sent on
  upload. If the server runs in `protect-writes` or `closed` mode, this needs
  **`write`** scope (a static `write:` token or an `mllm_` API key). Always pass
  it from a secret.
- **`--share`** uploads the completed run and prints `View run: <url>`; the URL
  uses the server's `MEASURELLM_PUBLIC_URL`.

On GitHub Actions the CLI automatically enriches the uploaded run with git
(branch, commit, dirty flag) and CI (provider + run URL) metadata, so shared runs
are traceable back to the workflow.

**Shared cache for CI.** Multiple CI jobs can share provider outputs through the
server's content-addressed cache (`/api/v1/cache/*`), which cuts cost and time on
reruns. The client side — the `http` cache backend, `MEASURELLM_SERVER_URL`, and
`cache_salt` — is documented in [`./caching.md`](./caching.md).

---

## This repo's CI (`ci.yml`)

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs on pushes to
`master`/`main` and on every PR. Superseded runs on the same ref are cancelled to
save minutes, and the workflow has read-only repo permissions.

| Job              | What it runs | What it guards |
|------------------|--------------|----------------|
| **fmt**          | `cargo fmt --all --check` | Formatting. |
| **clippy**       | `cargo clippy --workspace --all-targets -- -D warnings` | Lints as hard errors. |
| **test**         | `cargo test --workspace` | The Rust test suite. |
| **web**          | `pnpm -C web install --frozen-lockfile`, `pnpm -C web lint`, `pnpm -C web build`, `pnpm -C web test` | The web UI lints (`--max-warnings=0`), builds, and its vitest suite passes. |
| **schema-check** | Regenerates `measurellm schema config` and `diff`s it against the committed `measurellm.schema.json` | The checked-in JSON Schema hasn't drifted (run `mise run schema` to fix). |
| **gen-types-check** | Regenerates the TS DTOs into `web/src/api/generated` and diffs | Generated TypeScript types are current. Hard-fails if the dir is missing/uncommitted or drifts (run `mise run gen-types` and commit). |
| **musl-build**   | Static `cargo build --release -p measurellm-cli` for `x86_64-unknown-linux-musl` (native) | The fully static binary links on x86_64. aarch64 is not built here — it's verified at release time (see `release.yml`), where the cross toolchain is set up. |

The `schema-check` and `gen-types-check` jobs enforce the same generators as the
`schema` / `gen-types` mise tasks (`.mise/config.toml`) — keep those in sync when
you change them.

Every job installs its toolchain with [mise](https://mise.jdx.dev) via
[`jdx/mise-action`](https://github.com/jdx/mise-action) (pinned to a commit SHA),
reading the Rust/Node/pnpm versions from `.mise/config.toml` + `.mise/mise.lock`
so local and CI builds share one pinned toolchain. Rust compile caching stays on
`Swatinem/rust-cache`.

---

## Releases (`release.yml`)

[`.github/workflows/release.yml`](../.github/workflows/release.yml) runs on tags
matching `v*` and produces both binaries and a container image.

| Job          | What it does |
|--------------|--------------|
| **binaries** | Builds the web UI, then the static musl binary for `x86_64` (native) and `aarch64` (via `cross`); stages each as `measurellm-<target>` with a `.sha256`. aarch64 is `continue-on-error`. |
| **release**  | Downloads the staged binaries and publishes a GitHub Release with auto-generated notes and the `measurellm-*` assets attached. |
| **docker**   | Uses buildx + QEMU to build and push a **multi-arch** image (`linux/amd64,linux/arm64`) to `ghcr.io/perfectra1n/measurellm`. The Dockerfile is self-contained (it builds the UI + binary internally). |

Image tags come from `docker/metadata-action`: `{{version}}` (e.g. `1.2.3`),
`{{major}}.{{minor}}` (e.g. `1.2`), and `latest` — the latter only for
non-prerelease tags (a tag containing `-`, like `v1.2.3-rc.1`, does not move
`latest`).

Consume the image as described in [`./deploy.md`](./deploy.md):

```
ghcr.io/perfectra1n/measurellm:latest
```
