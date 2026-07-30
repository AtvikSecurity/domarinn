# Contributing to domarinn

Thanks for taking the time to contribute. 🎉

These are guidelines, not laws — use your judgement, and propose changes to them in a pull request if something here gets in the way.

- [Code of conduct](#code-of-conduct)
- [AI usage policy](#ai-usage-policy)
- [Getting set up](#getting-set-up)
- [The one command that matters](#the-one-command-that-matters)
- [Things that will fail CI](#things-that-will-fail-ci)
- [Submitting an issue](#submitting-an-issue)
- [Naming a pull request](#naming-a-pull-request)
- [Submitting a pull request](#submitting-a-pull-request)
- [CI, releases and the container image](#ci-releases-and-the-container-image)

## Code of conduct

See [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md). Found a security issue? Do **not** open an issue — see [SECURITY.md](./SECURITY.md).

## AI usage policy

domarinn is a harness for evaluating language models, so it would be strange to ban them. We ask for **disclosure and accountability**, not abstinence:

1. **Disclose it.** If AI wrote or substantially shaped any part of a contribution, say so in the pull request. The template has a line for it.
2. **You are the author.** You are responsible for every line you submit. A reviewer may ask why any of it is the way it is, and "the model wrote it" is not an answer. If you cannot explain a change, do not submit it.
3. **Review before you send.** Read the whole diff yourself first. Untested, unread generated code wastes reviewer time, and reviewer time is the scarcest thing this project has.
4. **Write your own prose.** Issues, PR descriptions, and review replies should be yours. We would rather read three blunt sentences you wrote than six polished paragraphs you did not.

## Getting set up

The toolchain is managed by [mise](https://mise.jdx.dev) — Rust, Node, pnpm, and the lint tools are all pinned in [`.mise/config.toml`](./.mise/config.toml) and checksummed in `.mise/mise.lock`. You should not need to install any of them yourself.

```sh
git clone https://github.com/AtvikSecurity/domarinn
cd domarinn
mise install     # installs the pinned toolchain and the git hooks
```

`mise install` also installs the [lefthook](https://lefthook.dev) pre-commit hooks, which run the shared formatters and lint the workflows with [zizmor](https://github.com/zizmorcore/zizmor).

Useful tasks (`mise tasks` lists them all):

| Task                  | What it does                                                   |
| --------------------- | -------------------------------------------------------------- |
| `mise run build`      | Build the web UI, then the release binary with the UI embedded |
| `mise run dev`        | Run the server + API on `:8321`                                |
| `mise run test`       | `cargo test --workspace`                                       |
| `mise run lint`       | clippy (as errors) + a formatting check                        |
| `mise run fmt`        | Auto-format                                                    |
| `mise run schema`     | Regenerate `domarinn.schema.json`                              |
| `mise run gen-types`  | Regenerate the TypeScript DTOs from the Rust types             |
| `mise run docs`       | Build the documentation site into `site/`                      |
| `mise run docs-serve` | Serve the docs locally with live reload                        |

## The one command that matters

```sh
mise run ci
```

Every CI job is a mise task, and the workflow invokes those tasks — so this runs byte-for-byte what CI runs. If it passes locally it passes in CI, with one caveat: `musl-build` needs a musl C toolchain on your host (`musl-tools` on Debian/Ubuntu) for rusqlite's bundled SQLite.

## Things that will fail CI

Two of the gates catch _generated_ files drifting from their sources, which is the failure most likely to surprise you:

- **`schema-check`** — you changed a config type but didn't regenerate the JSON Schema. Fix: `mise run schema`, then commit `domarinn.schema.json`.
- **`gen-types-check`** — you changed a DTO but didn't regenerate the TypeScript types. Fix: `mise run gen-types`, then commit `web/src/api/generated/`. Untracked new files count as drift too.

A third gate catches documentation drifting from the examples it shows:

- **the examples harness** (`crates/domarinn-cli/tests/examples.rs`) — every directory under `examples/` is run end to end against the real binary, and must appear in `crates/domarinn-cli/tests/examples/table.rs`, be transcluded by some page under `docs/`, _and_ be listed in the examples index.

**Adding an example is four steps, and CI names whichever one you missed:**

1. Create `examples/NN-kebab-name/domarinn.yaml`.
2. Add a row to `crates/domarinn-cli/tests/examples/table.rs` stating its exit code, cell tallies and case ids.
3. Transclude it from a page: `--8<-- "examples/NN-kebab-name/domarinn.yaml"`.
4. Add a numbered row to the table in `docs/examples/index.md`, linking the anchor on the page that transcludes it — the docs' front door, which the transclusion guard cannot see because the index transcludes nothing itself.

The point is that a documentation page and its test read the _same bytes_, so a page cannot describe a suite that no longer works. Docs never contain a copy of a full example — only the transclusion.

Also: `clippy` runs with `-D warnings`, and the web lint runs with `--max-warnings=0`. There is no warning budget.

## Submitting an issue

Search the tracker first — it may already exist, possibly with a workaround.

For bugs, please include a **minimal reproduction**: the smallest `domarinn.yaml` and command that shows the problem. Providers and models vary enormously, and without a reproduction we usually cannot tell a domarinn bug from a model being a model. Issues that go stale waiting for one get closed.

## Naming a pull request

**The title of your pull request must be a [Conventional Commit](https://www.conventionalcommits.org/en/v1.0.0/).** This is not bookkeeping: the repo squash-merges, so your PR title becomes the commit on `main`, and [Release Please](https://github.com/googleapis/release-please) parses those commits to decide the next version and write the changelog. A mistyped title ships the wrong version number.

```
<type>[(optional scope)][!]: <description>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`. Scopes in use: `core`, `types`, `protocol`, `cache`, `server`, `cli`, `web`, `logging`, `config`, `exec`, `docker`, `mise`, `deps`.

A trailing `!` marks a breaking change. While domarinn is pre-`1.0`, `feat` and `fix` bump the patch version and `!` bumps the minor.

Examples:

- `feat(cli): add --summary-md output`
- `fix(server): reject an SSO login with no email claim`
- `refactor(core)!: rename the assertion trait`

> We do **not** require conventional commits on individual commits inside a PR —
> only the title. If your PR is a single commit, GitHub already uses that
> commit's message as the title, so writing it conventionally gets both for
> free.

## Submitting a pull request

1. Check for an existing PR covering the same thing.
2. For anything large, open an issue first so the design can be discussed before you write it. Nobody enjoys rejecting a finished branch.
3. Fork, branch, and make your change. Add tests.
4. Run `mise run ci` and make it pass.
5. Open the PR with a conventional-commit title and fill in the template.

Reviews are open to anyone — please do review each other's work — but only maintainers can approve and merge.

## CI, releases and the container image

`mise run ci` above is what a contributor runs locally. This is what actually executes those gates on a PR, what happens once one merges, and how a release turns into published binaries and a container image.

### This repo's CI (`ci.yml`)

[`.github/workflows/ci.yml`](https://github.com/AtvikSecurity/domarinn/blob/main/.github/workflows/ci.yml) runs on pushes to `main` and on every PR. Superseded runs on the same ref are cancelled to save minutes, and the workflow has read-only repo permissions.

Every CI gate is a [mise task](https://github.com/AtvikSecurity/domarinn/blob/main/.mise/config.toml), and the workflow invokes those tasks — so `mise run <task>` locally runs byte-for-byte what CI runs, and **`mise run ci` runs the entire matrix in one go**.

| Job                 | Task it runs                                                                | What it guards                                                                                                                                                                                                                                                                                     |
| ------------------- | --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **fmt**             | `mise run fmt-check` (`cargo fmt --all --check`)                            | Formatting.                                                                                                                                                                                                                                                                                        |
| **clippy**          | `mise run clippy` (`cargo clippy --workspace --all-targets -- -D warnings`) | Lints as hard errors.                                                                                                                                                                                                                                                                              |
| **test**            | `mise run test` (`cargo test --workspace`)                                  | The Rust test suite.                                                                                                                                                                                                                                                                               |
| **web**             | `mise run web-install`, `web-lint`, `web-build`, `web-test`                 | The web UI installs from the frozen lockfile, lints (`--max-warnings=0`), builds, and its vitest suite passes.                                                                                                                                                                                     |
| **schema-check**    | `mise run schema-check`                                                     | The checked-in JSON Schema hasn't drifted (run `mise run schema` to fix).                                                                                                                                                                                                                          |
| **gen-types-check** | `mise run gen-types-check`                                                  | Generated TypeScript types are current. Hard-fails if the dir is missing/uncommitted or drifts (run `mise run gen-types` and commit).                                                                                                                                                              |
| **musl-build**      | `mise run musl-build`                                                       | The fully static binary links, on both shipped targets. Runs twice: `x86_64-unknown-linux-musl` on `ubuntu-24.04` and `aarch64-unknown-linux-musl` natively on `ubuntu-24.04-arm` — no cross-compiler, no QEMU. Set `MUSL_TARGET` to pick a triple locally (needs a musl C toolchain on the host). |
| **workflow-lint**   | `mise run workflow-lint`                                                    | The workflows pass [zizmor](https://github.com/zizmorcore/zizmor). The same check runs pre-commit via lefthook, but a fork PR never runs our hooks — this job is what gates one.                                                                                                                   |

Every job installs its toolchain with [mise](https://mise.jdx.dev) via [`jdx/mise-action`](https://github.com/jdx/mise-action) (pinned to a commit SHA), reading the Rust/Node/pnpm versions from `.mise/config.toml` + `.mise/mise.lock` so local and CI builds share one pinned toolchain. Rust compile caching stays on `Swatinem/rust-cache`.

### Releases

Releases are automated end to end. Nobody edits a version number by hand, and nobody pushes a tag by hand.

#### How a release happens

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

#### Where the version actually lives

`Cargo.toml`'s `[workspace.package] version` is the single source of truth; all six crates inherit it with `version.workspace = true`, and `domarinn_core::VERSION` (`env!("CARGO_PKG_VERSION")`) carries it to `--version`, the `/api/v1/meta` endpoint, the web UI footer, and every cache entry.

Release Please updates it through a **TOML extra-file updater** aimed at `$.workspace.package.version`, not through its built-in `rust` release type. That is not a stylistic choice: the `rust` strategy throws on a virtual workspace manifest (no `[package]` section), and the `cargo-workspace` plugin rejects members whose version is inherited rather than literal.

The consequence is that `Cargo.lock` is not updated by Release Please, so a second job in `release-please.yml` runs `cargo update --workspace` on the release branch and pushes the result. Without it, every `--locked` build (`mise run install`, `install-cli`, `install-musl`, and the `domarinn-eval` action's from-source fallback) would fail against the released tag.

#### `release.yml`

| Job          | What it does                                                                                                                                                                                                                           |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **binaries** | Builds the web UI, then the static musl binary for `x86_64` (on `ubuntu-24.04`) and `aarch64` (natively on `ubuntu-24.04-arm`). Neither leg may fail.                                                                                  |
| **sbom**     | Catalogues the dependency graph into one SPDX document for the whole release.                                                                                                                                                          |
| **upload**   | Writes the checksum manifest, signs every artifact with keyless cosign, then attaches them to the published release with `gh release upload --clobber`. It does **not** generate release notes — Release Please owns the release body. |

#### What a release publishes

Eight assets, whatever the number of targets:

| Asset                              | Notes                                                                                                                    |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `domarinn_<version>_linux_<arch>`  | Fully static musl binary, web UI embedded. One per arch (`amd64`, `arm64`)                                               |
| `domarinn_<version>.spdx.json`     | SPDX, ~900 packages across the Rust and npm graphs. **One per release, not per arch**                                    |
| `domarinn_<version>_checksums.txt` | Covers every artifact above. Bare filenames inside, so `sha256sum --check` works from any directory                      |
| `*.sigstore.json`                  | A cosign bundle for each of the above — the bundles are not themselves checksummed, since each carries its own integrity |

So for `0.2.0`:

```
domarinn_0.2.0_linux_amd64          domarinn_0.2.0_linux_amd64.sigstore.json
domarinn_0.2.0_linux_arm64          domarinn_0.2.0_linux_arm64.sigstore.json
domarinn_0.2.0.spdx.json            domarinn_0.2.0.spdx.json.sigstore.json
domarinn_0.2.0_checksums.txt        domarinn_0.2.0_checksums.txt.sigstore.json
```

**The version is always the second underscore-separated field**, so a file in `~/Downloads` identifies itself. The arch is `amd64`/`arm64` rather than the Rust triple: `x86_64-unknown-linux-musl` puts the literal word _unknown_ — the vendor field — exactly where a reader looks for a version. `musl` is not in the name either; that these are fully static is a documented property, not a filename.

The cost is that `releases/latest/download/<name>` no longer exists, because GitHub has no wildcard there. Consumers resolve the tag first:

```sh
ver=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
  https://github.com/AtvikSecurity/domarinn/releases/latest | sed 's#.*/tag/##')
```

That redirect needs no API token and is not rate-limited the way unauthenticated `api.github.com` is. `README.md`, [`docs/start/install.md`](docs/start/install.md) and [`domarinn-eval`](https://github.com/AtvikSecurity/domarinn/blob/main/.github/actions/domarinn-eval/action.yml) all do this; the action additionally falls back to the old unversioned asset name so pinning `0.1.3` or earlier still downloads rather than rebuilding from source.

Four decisions worth knowing if you touch this workflow:

- **The SBOM has a job of its own, on an unbuilt checkout.** Not fussiness — two things force it. `[profile.release] strip = true`, so scanning the shipped binary yields _one_ package instead of ~890; the graph only survives in the lockfiles. And syft on a built tree would additionally crawl `target/` and `node_modules/`, taking far longer for a worse result. A separate job makes "nothing has been built here" structural rather than a comment to respect.
- **One SBOM, not one per target.** It is generated from `Cargo.lock` and `web/pnpm-lock.yaml`, neither of which mentions a target triple — so the old per-leg copies were the same 890-package document under two names, implying a per-target dependency graph that does not exist.
- **The checksums are generated from inside `dist/`.** `sha256sum` records the path exactly as given, so running it from the repo root bakes in a `dist/` prefix that does not exist for whoever downloads the release, and their `sha256sum --check` fails. This shipped broken in `0.1.1`.
- **The operands are listed, not globbed.** The manifest must not appear inside itself, and `shopt -s failglob` turns a missing artifact into a failed release rather than a silently short manifest.

Signing is keyless: the OIDC identity of the workflow is the signer, so there is no key to manage or rotate. See [`docs/start/install.md`](docs/start/install.md#verifying-a-download) for the verification commands.

### Container image (`docker.yml`)

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

Consume the image as described in [`docs/guides/self-host.md`](docs/guides/self-host.md):

```
ghcr.io/atviksecurity/domarinn:rolling
```
