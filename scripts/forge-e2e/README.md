# Forge e2e harness

An end-to-end test of the Gitea and Forgejo copies of the `domarinn-eval`
composite action (`.gitea/actions/domarinn-eval`, `.forgejo/actions/domarinn-eval`)
on a real forge with its real runner. `crates/domarinn-cli/tests/action_contract.rs`
runs the same shell against stub binaries; this runs it the way a user does.

```sh
mise run forge-e2e            # both forges
mise run forge-e2e gitea      # or just one
KEEP=1 mise run forge-e2e forgejo   # leave the stack up to poke at
```

Needs Linux, Docker with the compose plugin, `curl`, `git` and `python3`.
The `Forge e2e` workflow (`.github/workflows/forge-e2e.yml`) runs it as a
matrix on pull requests that touch the actions, this directory, or the CLI
code the summary comes from.

## What it pins

| Piece                             | Image                                     |
| --------------------------------- | ----------------------------------------- |
| Gitea                             | `docker.io/gitea/gitea:28.2.0`            |
| Gitea runner                      | `docker.io/gitea/runner:5.2.4`            |
| Forgejo                           | `data.forgejo.org/forgejo/forgejo:17.0.1` |
| Forgejo runner                    | `data.forgejo.org/forgejo/runner:13.2.0`  |
| Job image (label `ubuntu-latest`) | `docker.io/library/node:24-bookworm`      |

All in `compose.yaml` (tag + digest) except the job image, which is a runner
label and lives in `run.sh` behind a `# renovate:` annotation. Renovate bumps
all of them.

## What happens

`run.sh`, per forge:

1. Gets a static x86_64 `domarinn`: `DOMARINN_E2E_BIN` if set, else
   `mise run musl-build` when `musl-gcc` is on `PATH`, else a Docker build
   (`Dockerfile.musl-build`, Alpine's musl Rust).
2. Boots the forge headlessly (`INSTALL_LOCK`, SQLite), creates an admin
   `e2e` and an unprivileged `reviewer` with the forge's own CLI, and
   registers the runner: a registration token on Gitea, an offline shared
   secret (`forgejo-cli actions register`) on Forgejo.
3. Starts the runner in **docker mode**: every job runs in a stock
   `node:24-bookworm` container on the host's Docker, attached to the compose
   network, where the forge's ROOT_URL (`http://<forge>:3000`) resolves. See
   the header of `compose.yaml` for why not host mode.
4. Pushes the action files from this working tree to `e2e/domarinn` on the
   instance and publishes the binary there as release `<version>` with the
   real asset name `domarinn_<version>_linux_amd64`, after an older, empty
   `0.0.1` release.

Then `scenario.py` drives the pull request and asserts everything through
the forge's REST API. The consumer workflow (`fixture/eval.yml`) uses the
action by URL (`uses: http://<forge>:3000/e2e/domarinn/.<forge>/actions/domarinn-eval@main`)
with `release-base-url` pointing at that repository and `version: latest`, so
the binary arrives through the forge release API exactly as on an air-gapped
instance.

| Step           | Asserted                                                                                                                                                                                                                                                                                                                   |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Warm-up        | A run in another repository first, so the consumer's run ids and run numbers differ.                                                                                                                                                                                                                                       |
| PR opened      | A reviewer comment that _starts_ with `<!-- domarinn-eval -->` is posted before the run. The job succeeds; exactly one comment comes from the job token, starts with the marker and carries the summary; the reviewer's comment is unchanged.                                                                              |
| Second commit  | After a reviewer quote-reply (marker not at the start), the run updates that same comment (same id, new content); both reviewer comments unchanged.                                                                                                                                                                        |
| Failing commit | The job fails (exit 1, `fail-on-regression: true`); the same comment now reports the failure.                                                                                                                                                                                                                              |
| Every run      | The action's outputs (`exit-code`, `passed`, `failed`, `total`), read from the job log; `latest` resolved to the published version; no comment/summary warnings; the `domarinn-results` artifact holds `results.xml` and `summary.md`; the summary's `CI run` link answers 200 and opens this run's page, not another one. |

Gitea's artifacts API lists only v4 artifacts and the Gitea action uploads
with `upload-artifact` v3, so there the artifact is fetched from the run
page's download route instead. Forgejo's API lists it.

Logs (forge, runner, every job log read) land in `.work/<forge>/logs/`, and
CI uploads them when a forge fails. The stack is torn down with
`docker compose down -v` unless `KEEP=1`.
