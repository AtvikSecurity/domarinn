# CI integration

domarinn is built to gate pull requests: it runs an eval suite, compares
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
- [Container image (`docker.yml`)](#container-image-dockeryml)

---

## The exit-code contract

Every domarinn run exits with a code your CI can branch on. **`3` (infra) wins
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
domarinn run \
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
domarinn run --against latest --format junit --out results.xml --summary-md summary.md
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

### Stable output in CI

CI logs are deterministic: domarinn detects that its output is captured (not a
terminal) and drops everything cosmetic. The **live progress bar is suppressed**
on a non-TTY stderr (so no carriage returns or redraws clutter the log), and
**human output is never colored** without a terminal. `stdout` — your `json`,
`jsonl`, or `junit` report — is byte-for-byte identical whether or not a terminal
is attached. If you ever need to force it, `NO_COLOR=1`, `--color never`, and
`--no-progress` all guarantee plain, stable output regardless of environment.

---

## The reusable action

A composite action lives at
[`.github/actions/domarinn-eval/action.yml`](../.github/actions/domarinn-eval/action.yml).
It resolves a `domarinn` binary, runs your suite, uploads the JUnit report +
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
      - uses: AtvikSecurity/domarinn/.github/actions/domarinn-eval@v1
        with:
          config: eval/domarinn.yaml
          against: latest
```

### Inputs

| Input                | Default             | Purpose |
|----------------------|---------------------|---------|
| `config`             | `domarinn.yaml`   | Suite file, or a directory containing `domarinn.yaml`. |
| `binary-path`        | `""`                | Path to a prebuilt `domarinn`. When set, binary resolution stops here (no download, no build). |
| `version`            | `latest`            | Release tag to download the binary from (e.g. `v0.1.0`), or `latest`. Used only when `binary-path` is empty. |
| `server-url`         | `""`                | Results server base URL. When set, the run is uploaded with `--share` (exported as `DOMARINN_SERVER_URL`). |
| `token`              | `""`                | Bearer token for the results server (exported as `DOMARINN_TOKEN`). Pass a **secret**, never a literal. |
| `against`            | `""`                | Baseline to diff against (`latest`, a run id, or a `result.json` path). Empty disables the diff. A regression makes the CLI exit 1. |
| `fail-on-regression` | `"true"`            | If `true`, exit 1 (assertion/regression) fails the check. If `false`, exit 1 is a warning only. Exit 2 and 3 **always** fail. |
| `comment`            | `"true"`            | Post/update the summary comment on the PR. |
| `artifact-name`      | `"domarinn-results"` | Name of the uploaded artifact holding `results.xml` + `summary.md`. |

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
   `domarinn-<target>` from the repo's GitHub Releases (`version` or `latest`,
   arch auto-detected) → **fallback** to building from source with `cargo`
   (`cargo install --path crates/domarinn-cli` if this repo is checked out,
   else `cargo install --git …`). The cargo fallback requires a Rust toolchain on
   the runner — add `dtolnay/rust-toolchain` before this action if you rely on it.
2. **Run the suite:**
   `domarinn run <config> --format junit --out results.xml --summary-md
   summary.md`, appending `--against <against>` and `--share` when those inputs
   are set. It captures the exit code without aborting so the later steps still
   run.
3. **Parse headline numbers** — `tests`/`failures`/`errors` from the JUnit
   `<testsuite>` element, and `regressed` from the summary's `| Newly failing |
   N |` row.
4. **Upload** `results.xml` + `summary.md` as an artifact (**always**, even on
   failure), and append the summary to the job's step summary.
5. **Comment on the PR** — creates or updates one comment (matched by a hidden
   `<!-- domarinn-eval -->` marker) so repeated pushes don't spam the thread.
   Skipped unless `comment: true` and the event is a `pull_request`.
6. **Gate** on the exit code: `0` passes; `1` fails **only when
   `fail-on-regression` is true** (otherwise a warning); `2` and `3` always fail.

### Advanced usage

```yaml
- uses: AtvikSecurity/domarinn/.github/actions/domarinn-eval@v1
  with:
    config: eval/
    against: latest
    fail-on-regression: "true"
    server-url: https://domarinn.example.com   # enables --share
    token: ${{ secrets.DOMARINN_TOKEN }}        # write-scoped token
```

---

## Uploading CI runs to a shared server

Point CI at a shared [server](./server.md) so every eval is browsable and each PR
gets a durable link, and so runs can share a provider cache.

- **`server-url` / `DOMARINN_SERVER_URL`** — the server base URL. Setting it
  makes the run upload with **`--share`**.
- **`DOMARINN_TOKEN`** (the action's `token` input) — a bearer token sent on
  upload. If the server runs in `protect-writes` or `closed` mode, this needs
  **`write`** scope (a static `write:` token or an `domarinn_` API key). Always pass
  it from a secret.
- **`--share`** uploads the completed run and prints `View run: <url>`; the URL
  uses the server's `DOMARINN_PUBLIC_URL`.

On GitHub Actions the CLI automatically enriches the uploaded run with git
(branch, commit, dirty flag) and CI (provider + run URL) metadata, so shared runs
are traceable back to the workflow.

**Shared cache for CI.** Multiple CI jobs can share provider outputs through the
server's content-addressed cache (`/api/v1/cache/*`), which cuts cost and time on
reruns. The client side — the `http` cache backend, `DOMARINN_SERVER_URL`, and
`cache_salt` — is documented in [`./caching.md`](./caching.md).

---

## This repo's CI (`ci.yml`)

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs on pushes to
`main` and on every PR. Superseded runs on the same ref are cancelled to
save minutes, and the workflow has read-only repo permissions.

Every CI gate is a [mise task](../.mise/config.toml), and the workflow invokes
those tasks — so `mise run <task>` locally runs byte-for-byte what CI runs, and
**`mise run ci` runs the entire matrix in one go**.

| Job              | Task it runs | What it guards |
|------------------|--------------|----------------|
| **fmt**          | `mise run fmt-check` (`cargo fmt --all --check`) | Formatting. |
| **clippy**       | `mise run clippy` (`cargo clippy --workspace --all-targets -- -D warnings`) | Lints as hard errors. |
| **test**         | `mise run test` (`cargo test --workspace`) | The Rust test suite. |
| **web**          | `mise run web-install`, `web-lint`, `web-build`, `web-test` | The web UI installs from the frozen lockfile, lints (`--max-warnings=0`), builds, and its vitest suite passes. |
| **schema-check** | `mise run schema-check` | The checked-in JSON Schema hasn't drifted (run `mise run schema` to fix). |
| **gen-types-check** | `mise run gen-types-check` | Generated TypeScript types are current. Hard-fails if the dir is missing/uncommitted or drifts (run `mise run gen-types` and commit). |
| **musl-build**   | `mise run musl-build` | The fully static binary links on x86_64 (needs a musl C toolchain on the host). aarch64 is not built here — it's verified at release time (see `release.yml`), where the cross toolchain is set up. |

Every job installs its toolchain with [mise](https://mise.jdx.dev) via
[`jdx/mise-action`](https://github.com/jdx/mise-action) (pinned to a commit SHA),
reading the Rust/Node/pnpm versions from `.mise/config.toml` + `.mise/mise.lock`
so local and CI builds share one pinned toolchain. Rust compile caching stays on
`Swatinem/rust-cache`.

---

## Releases (`release.yml`)

[`.github/workflows/release.yml`](../.github/workflows/release.yml) runs on tags
matching `v*` and produces the release binaries. (The container image is built
separately — see [`docker.yml`](#container-image-dockeryml) below.)

| Job          | What it does |
|--------------|--------------|
| **binaries** | Builds the web UI, then the static musl binary for `x86_64` (native) and `aarch64` (via `cross`); stages each as `domarinn-<target>` with a `.sha256`. aarch64 is `continue-on-error`. |
| **release**  | Downloads the staged binaries and publishes a GitHub Release with auto-generated notes and the `domarinn-*` assets attached. |

---

## Container image (`docker.yml`)

[`.github/workflows/docker.yml`](../.github/workflows/docker.yml) builds the
container image and pushes it to the Gitea registry as
`ghcr.io/atviksecurity/domarinn`, mirroring the
bake/digest/merge pipeline from the AtvikSecurity/containers repo. The build
itself is described by [`docker-bake.hcl`](../docker-bake.hcl); the Dockerfile
is self-contained (it builds the UI + binary internally).

| Job       | What it does |
|-----------|--------------|
| **plan**  | Derives tags/labels once with `docker/metadata-action` and stages the resulting bake file as an artifact. |
| **build** | One leg per platform (currently `linux/amd64`): `docker/bake-action` builds the `image` target and pushes it **by digest** (untagged), with a registry-backed build cache at `ghcr.io/atviksecurity/build_cache:domarinn-<arch>`. |
| **merge** | Stitches the per-platform digests into one tagged manifest list with `docker buildx imagetools create`. |

It runs on pushes to `main`, on `v*` tags, and via `workflow_dispatch`.
Registry auth uses the `GITEA_USERNAME` / `GITEA_TOKEN` repo secrets, so the
workflow token stays read-only.

Image tags come from `docker/metadata-action`: `rolling` tracks `main`, and a
`v*` tag produces `{{version}}` (e.g. `1.2.3`), `{{major}}.{{minor}}` (e.g.
`1.2`), and `{{major}}` (e.g. `1`). There is **no `latest` tag** — track
`rolling` for the tip of main or a semver tag for releases.

Consume the image as described in [`./deploy.md`](./deploy.md):

```
ghcr.io/atviksecurity/domarinn:rolling
```
