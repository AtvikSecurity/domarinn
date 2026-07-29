# CI integration

domarinn is built to gate pull requests: it runs an eval suite, compares against a baseline, writes machine-readable reports, and — crucially — returns an **exit code that distinguishes "the model regressed" from "the harness broke".** This page covers gating a PR, the reusable GitHub Action, this repo's own CI/CD, and uploading CI runs to a shared server.

- [The exit-code contract](#the-exit-code-contract)
- [Gating a PR by hand](#gating-a-pr-by-hand)
- [The reusable action](#the-reusable-action)
- [The `ci-summary` command](#the-ci-summary-command)
- [Uploading CI runs to a shared server](#uploading-ci-runs-to-a-shared-server)
- [This repo's CI (`ci.yml`)](#this-repos-ci-ciyml)
- [Releases](#releases)
- [Container image (`docker.yml`)](#container-image-dockeryml)

---

## The exit-code contract

Every domarinn run exits with a code your CI can branch on. **`3` (infra) wins over `1` (assertion)** when both happen in one run — a broken harness must never masquerade as a passing eval, nor should it be blamed on the PR author.

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

The [reusable action](#the-reusable-action) below does exactly this, plus the report upload and PR comment, so you usually don't hand-roll it.

### Stable output in CI

CI logs are deterministic: domarinn detects that its output is captured (not a terminal) and drops everything cosmetic. The **live progress bar is suppressed** on a non-TTY stderr (so no carriage returns or redraws clutter the log), and **human output is never colored** without a terminal. `stdout` — your `json`, `jsonl`, or `junit` report — is byte-for-byte identical whether or not a terminal is attached. If you ever need to force it, `NO_COLOR=1`, `--color never`, and `--no-progress` all guarantee plain, stable output regardless of environment.

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

## Uploading CI runs to a shared server

Point CI at a shared [server](./server.md) so every eval is browsable and each PR gets a durable link, and so runs can share a provider cache.

- **`server-url` / `DOMARINN_SERVER_URL`** — the server base URL. Setting it makes the run upload with **`--share`**.
- **`DOMARINN_TOKEN`** (the action's `token` input) — a bearer token sent on upload. Unless the server is explicitly in `open` mode, this needs **`write`** scope (a static `write:` token or an `domarinn_` API key). Always pass it from a secret.
- **`--share`** uploads the completed run and prints `View run: <url>`; the URL uses the server's `DOMARINN_PUBLIC_URL`.

On GitHub Actions the CLI automatically enriches the uploaded run with git (branch, commit, dirty flag) and CI (provider + run URL) metadata, so shared runs are traceable back to the workflow.

**Shared cache for CI.** Multiple CI jobs can share every request domarinn makes — provider responses, judge verdicts, embeddings — through the server's content-addressed cache (`/api/v1/cache/*`), which cuts cost and time on reruns. The client side is `cache.backend: layered` plus `DOMARINN_SERVER_URL`, documented in [`./caching.md`](./caching.md#backends).

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

The first CI run after upgrading to 0.5 re-files what a 0.4-era cache holds under the new keys, automatically and once — see [Upgrading to 0.5](./caching.md#upgrading-to-05). A job that pins `--cache-only` should run once in the default read-write mode first, or `similar` assertions will hard-error: their 0.4 entries are the one shape that cannot be adopted.

---

## This repo's CI (`ci.yml`)

[`.github/workflows/ci.yml`](https://github.com/AtvikSecurity/domarinn/blob/main/.github/workflows/ci.yml) runs on pushes to `main` and on every PR. Superseded runs on the same ref are cancelled to save minutes, and the workflow has read-only repo permissions.

Every CI gate is a [mise task](https://github.com/AtvikSecurity/domarinn/blob/main/.mise/config.toml), and the workflow invokes those tasks — so `mise run <task>` locally runs byte-for-byte what CI runs, and **`mise run ci` runs the entire matrix in one go**.

| Job              | Task it runs | What it guards |
|------------------|--------------|----------------|
| **fmt**          | `mise run fmt-check` (`cargo fmt --all --check`) | Formatting. |
| **clippy**       | `mise run clippy` (`cargo clippy --workspace --all-targets -- -D warnings`) | Lints as hard errors. |
| **test**         | `mise run test` (`cargo test --workspace`) | The Rust test suite. |
| **web**          | `mise run web-install`, `web-lint`, `web-build`, `web-test` | The web UI installs from the frozen lockfile, lints (`--max-warnings=0`), builds, and its vitest suite passes. |
| **schema-check** | `mise run schema-check` | The checked-in JSON Schema hasn't drifted (run `mise run schema` to fix). |
| **gen-types-check** | `mise run gen-types-check` | Generated TypeScript types are current. Hard-fails if the dir is missing/uncommitted or drifts (run `mise run gen-types` and commit). |
| **musl-build**   | `mise run musl-build` | The fully static binary links, on both shipped targets. Runs twice: `x86_64-unknown-linux-musl` on `ubuntu-24.04` and `aarch64-unknown-linux-musl` natively on `ubuntu-24.04-arm` — no cross-compiler, no QEMU. Set `MUSL_TARGET` to pick a triple locally (needs a musl C toolchain on the host). |
| **workflow-lint** | `mise run workflow-lint` | The workflows pass [zizmor](https://github.com/zizmorcore/zizmor). The same check runs pre-commit via lefthook, but a fork PR never runs our hooks — this job is what gates one. |

Every job installs its toolchain with [mise](https://mise.jdx.dev) via [`jdx/mise-action`](https://github.com/jdx/mise-action) (pinned to a commit SHA), reading the Rust/Node/pnpm versions from `.mise/config.toml` + `.mise/mise.lock` so local and CI builds share one pinned toolchain. Rust compile caching stays on `Swatinem/rust-cache`.

---

## Releases

Releases are automated end to end. Nobody edits a version number by hand, and nobody pushes a tag by hand.

### How a release happens

1. You merge a PR whose **title** is a conventional commit (`feat: …`, `fix: …`, `refactor!: …`). The title matters because the repo squash-merges, so the PR title becomes the commit on `main`.
2. [`release-please.yml`](https://github.com/AtvikSecurity/domarinn/blob/main/.github/workflows/release-please.yml) sees the new commit and opens (or updates) a standing **`chore(main): release X.Y.Z`** pull request containing the `CHANGELOG.md` entry and the version bump.
3. You merge that PR when you want to ship. Release Please tags the merge commit and publishes the GitHub Release.
4. [`release.yml`](https://github.com/AtvikSecurity/domarinn/blob/main/.github/workflows/release.yml) and [`docker.yml`](#container-image-dockeryml) both fire on `release: published` and attach the binaries and images.

Versions are **bare semver** — the tag is `0.2.0`, not `v0.2.0`.

While the project is pre-`1.0`, `feat` and `fix` bump the patch and a breaking change (`!`) bumps the minor, per `bump-minor-pre-major` / `bump-patch-for-minor-pre-major` in [`release-please-config.json`](https://github.com/AtvikSecurity/domarinn/blob/main/release-please-config.json).

> **Dependency bumps do not cut releases.** Renovate is configured
> ([`renovate.json5`](https://github.com/AtvikSecurity/domarinn/blob/main/renovate.json5)) to emit `chore(deps)` / `ci(...)` and
> never a `!` marker, so its commits ride along in the next human-authored
> release. To ship one urgently, land a hand-written `fix(deps): …` commit.

### Where the version actually lives

`Cargo.toml`'s `[workspace.package] version` is the single source of truth; all six crates inherit it with `version.workspace = true`, and `domarinn_core::VERSION` (`env!("CARGO_PKG_VERSION")`) carries it to `--version`, the `/api/v1/meta` endpoint, the web UI footer, and every cache entry.

Release Please updates it through a **TOML extra-file updater** aimed at `$.workspace.package.version`, not through its built-in `rust` release type. That is not a stylistic choice: the `rust` strategy throws on a virtual workspace manifest (no `[package]` section), and the `cargo-workspace` plugin rejects members whose version is inherited rather than literal.

The consequence is that `Cargo.lock` is not updated by Release Please, so a second job in `release-please.yml` runs `cargo update --workspace` on the release branch and pushes the result. Without it, every `--locked` build (`mise run install`, `install-cli`, `install-musl`, and the `domarinn-eval` action's from-source fallback) would fail against the released tag.

### `release.yml`

| Job          | What it does |
|--------------|--------------|
| **binaries** | Builds the web UI, then the static musl binary for `x86_64` (on `ubuntu-24.04`) and `aarch64` (natively on `ubuntu-24.04-arm`). Neither leg may fail. |
| **sbom**     | Catalogues the dependency graph into one SPDX document for the whole release. |
| **upload**   | Writes the checksum manifest, signs every artifact with keyless cosign, then attaches them to the published release with `gh release upload --clobber`. It does **not** generate release notes — Release Please owns the release body. |

### What a release publishes

Eight assets, whatever the number of targets:

| Asset | Notes |
|---|---|
| `domarinn_<version>_linux_<arch>` | Fully static musl binary, web UI embedded. One per arch (`amd64`, `arm64`) |
| `domarinn_<version>.spdx.json` | SPDX, ~900 packages across the Rust and npm graphs. **One per release, not per arch** |
| `domarinn_<version>_checksums.txt` | Covers every artifact above. Bare filenames inside, so `sha256sum --check` works from any directory |
| `*.sigstore.json` | A cosign bundle for each of the above — the bundles are not themselves checksummed, since each carries its own integrity |

So for `0.2.0`:

```
domarinn_0.2.0_linux_amd64          domarinn_0.2.0_linux_amd64.sigstore.json
domarinn_0.2.0_linux_arm64          domarinn_0.2.0_linux_arm64.sigstore.json
domarinn_0.2.0.spdx.json            domarinn_0.2.0.spdx.json.sigstore.json
domarinn_0.2.0_checksums.txt        domarinn_0.2.0_checksums.txt.sigstore.json
```

**The version is always the second underscore-separated field**, so a file in `~/Downloads` identifies itself. The arch is `amd64`/`arm64` rather than the Rust triple: `x86_64-unknown-linux-musl` puts the literal word *unknown* — the vendor field — exactly where a reader looks for a version. `musl` is not in the name either; that these are fully static is a documented property, not a filename.

The cost is that `releases/latest/download/<name>` no longer exists, because GitHub has no wildcard there. Consumers resolve the tag first:

```sh
ver=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
  https://github.com/AtvikSecurity/domarinn/releases/latest | sed 's#.*/tag/##')
```

That redirect needs no API token and is not rate-limited the way unauthenticated `api.github.com` is. `README.md`, [`getting-started`](./getting-started.md) and [`domarinn-eval`](https://github.com/AtvikSecurity/domarinn/blob/main/.github/actions/domarinn-eval/action.yml) all do this; the action additionally falls back to the old unversioned asset name so pinning `0.1.3` or earlier still downloads rather than rebuilding from source.

Four decisions worth knowing if you touch this workflow:

- **The SBOM has a job of its own, on an unbuilt checkout.** Not fussiness — two things force it. `[profile.release] strip = true`, so scanning the shipped binary yields *one* package instead of ~890; the graph only survives in the lockfiles. And syft on a built tree would additionally crawl `target/` and `node_modules/`, taking far longer for a worse result. A separate job makes "nothing has been built here" structural rather than a comment to respect.
- **One SBOM, not one per target.** It is generated from `Cargo.lock` and `web/pnpm-lock.yaml`, neither of which mentions a target triple — so the old per-leg copies were the same 890-package document under two names, implying a per-target dependency graph that does not exist.
- **The checksums are generated from inside `dist/`.** `sha256sum` records the path exactly as given, so running it from the repo root bakes in a `dist/` prefix that does not exist for whoever downloads the release, and their `sha256sum --check` fails. This shipped broken in `0.1.1`.
- **The operands are listed, not globbed.** The manifest must not appear inside itself, and `shopt -s failglob` turns a missing artifact into a failed release rather than a silently short manifest.

Signing is keyless: the OIDC identity of the workflow is the signer, so there is no key to manage or rotate. See [getting-started](./getting-started.md#verifying-a-download) for the verification commands.

---

## Container image (`docker.yml`)

[`.github/workflows/docker.yml`](https://github.com/AtvikSecurity/domarinn/blob/main/.github/workflows/docker.yml) publishes the container image to GHCR as `ghcr.io/atviksecurity/domarinn`.

The workflow itself is thin: it delegates to [`docker/github-builder`](https://github.com/docker/github-builder)'s reusable `build.yml`, which splits the platform list across runners (its default mapping sends `linux/arm64` to `ubuntu-24.04-arm`), merges the per-platform digests into one manifest list, generates an SBOM, and signs the result with keyless cosign. The Dockerfile is self-contained — it builds the UI and the binary internally.

It runs on pushes to `main`, on published releases, and via `workflow_dispatch`. Registry auth is the built-in `GITHUB_TOKEN`, so **no registry secrets are needed**; the job requests `packages: write` to push and `id-token: write` so cosign can mint an OIDC identity.

Image tags come from `docker/metadata-action`: `rolling` tracks `main`, and a published release produces `{{version}}` (e.g. `1.2.3`), `{{major}}.{{minor}}` (e.g. `1.2`), and `{{major}}` (e.g. `1`). There is **no `latest` tag** — track `rolling` for the tip of main or a semver tag for releases.

One thing the workflow cannot do for you: **GHCR package visibility is a setting on the package, not a workflow permission.** A package is private when it is first pushed and stays that way until someone flips it in `Packages → domarinn → Package settings → Change visibility`, no matter that the repository is public and `packages: write` is granted. While it is private, every `docker pull` in this documentation returns `401 UNAUTHORIZED` for anyone outside the org — including the versioned tags, which are pushed regardless. Check with `curl -s "https://ghcr.io/token?scope=repository:atviksecurity/domarinn:pull&service=ghcr.io"`: a token means public, `UNAUTHORIZED` means private.

Both `linux/amd64` and `linux/arm64` are published. Verify a pull with:

```console
$ docker buildx imagetools inspect ghcr.io/atviksecurity/domarinn:rolling
```

[`docker-bake.hcl`](https://github.com/AtvikSecurity/domarinn/blob/main/docker-bake.hcl) is **not** used by CI — it exists so `docker buildx bake` reproduces the image locally.

Consume the image as described in [`./deploy.md`](./deploy.md):

```
ghcr.io/atviksecurity/domarinn:rolling
```
