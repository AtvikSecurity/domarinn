#!/usr/bin/env bash
# End-to-end test of the Gitea and Forgejo copies of the domarinn-eval
# composite action, on a real forge with its real runner.
#
#   scripts/forge-e2e/run.sh [gitea|forgejo|all]     (or: mise run forge-e2e …)
#
# For each forge: boot it and its runner (compose.yaml), bootstrap them
# headlessly, publish a static domarinn as a release of an `e2e/domarinn`
# repository on that instance alongside the action files from this working
# tree, then hand over to scenario.py, which drives a pull request through the
# action and asserts the outcome through the forge's API. See README.md.
#
# Environment:
#   KEEP=1             leave the stack up afterwards (prints how to reach it)
#   DOMARINN_E2E_BIN   a prebuilt static x86_64 domarinn to publish; otherwise
#                      one is built (`mise run musl-build` when musl-gcc is on
#                      PATH, else in Docker via Dockerfile.musl-build)
#   DOCKER_SOCK        the Docker socket to hand the runner (default
#                      /var/run/docker.sock)
#   RUN_TIMEOUT        seconds to wait for each workflow run (default 900)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${HERE}/../.." && pwd)"
WORK_ROOT="${HERE}/.work"

# The image every job runs in, via the runner label `ubuntu-latest`. The stock
# Node image: Debian bookworm with bash, curl, git and python3, which is what
# the action and the fixture suite's exec provider need, and nothing added.
# renovate: datasource=docker depName=docker.io/library/node
JOB_IMAGE_TAG="24-bookworm"
JOB_IMAGE="docker.io/library/node:${JOB_IMAGE_TAG}"

log() { printf '\n==> %s\n' "$*" >&2; }
die() { printf 'forge-e2e: %s\n' "$*" >&2; exit 1; }

# --- The binary ------------------------------------------------------------

# Prints the path to a static x86_64 domarinn, building one if needed.
resolve_binary() {
  if [ -n "${DOMARINN_E2E_BIN:-}" ]; then
    [ -x "${DOMARINN_E2E_BIN}" ] || die "DOMARINN_E2E_BIN=${DOMARINN_E2E_BIN} is not an executable"
    printf '%s\n' "${DOMARINN_E2E_BIN}"
    return
  fi
  [ "$(uname -m)" = x86_64 ] || die "the harness publishes a linux_amd64 asset; set DOMARINN_E2E_BIN on $(uname -m)"
  if command -v musl-gcc >/dev/null 2>&1; then
    log "building the static binary with mise run musl-build"
    (cd "${ROOT}" && mise run musl-build) >&2
    printf '%s\n' "${ROOT}/target/x86_64-unknown-linux-musl/release/domarinn"
  else
    log "no musl-gcc on PATH: building the static binary in Docker (Dockerfile.musl-build)"
    docker buildx build -f "${HERE}/Dockerfile.musl-build" \
      --output "type=local,dest=${WORK_ROOT}/bin" "${ROOT}" >&2
    printf '%s\n' "${WORK_ROOT}/bin/domarinn"
  fi
}

# --- One forge -------------------------------------------------------------

compose() {
  docker compose -f "${HERE}/compose.yaml" -p "domarinn-forge-e2e-${FORGE}" --profile "${FORGE}" "$@"
}

# Runs the forge's own CLI inside its container, as its unprivileged user
# (uid 1000 in both images; the CLI refuses to run as root).
forge_cli() {
  docker exec -u 1000 "$(compose ps -q "${FORGE}")" "${FORGE}" "$@"
}

api() { # method path [json]
  local method="$1" path="$2"
  shift 2
  curl --fail-with-body -sS -u "${ADMIN_USER}:${ADMIN_PASS}" -X "${method}" \
    -H 'Content-Type: application/json' "${BASE}/api/v1${path}" "$@"
}

json_field() { # field — reads a JSON object on stdin
  python3 -c 'import json, sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"
}

# A git that ignores the caller's config (signing, hooks, default branch).
fgit() {
  GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    GIT_AUTHOR_NAME=e2e GIT_AUTHOR_EMAIL=e2e@example.invalid \
    GIT_COMMITTER_NAME=e2e GIT_COMMITTER_EMAIL=e2e@example.invalid \
    git -c init.defaultBranch=main -c core.hooksPath=/dev/null "$@"
}

dump_logs() {
  local dir="${WORK_DIR}/logs"
  mkdir -p "${dir}"
  compose ps -a >"${dir}/compose-ps.txt" 2>&1 || true
  compose logs --no-color "${FORGE}" >"${dir}/${FORGE}.log" 2>&1 || true
  compose logs --no-color "${FORGE}-runner" >"${dir}/${FORGE}-runner.log" 2>&1 || true
  log "logs for ${FORGE} are in ${dir}"
}

teardown() {
  local status=$?
  if [ "${status}" -ne 0 ]; then
    dump_logs
    tail -n 40 "${WORK_DIR}/logs/${FORGE}-runner.log" >&2 || true
  fi
  if [ "${KEEP:-0}" = 1 ]; then
    log "KEEP=1: ${FORGE} left running at ${BASE:-?} (${ADMIN_USER}/${ADMIN_PASS}); tear down with: FORGE=${FORGE} WORK_DIR=${WORK_DIR} docker compose -f ${HERE}/compose.yaml -p domarinn-forge-e2e-${FORGE} --profile ${FORGE} down -v"
  else
    compose down -v --remove-orphans >/dev/null 2>&1 || true
  fi
  return "${status}"
}

write_runner_config() {
  local network="domarinn-forge-e2e-${FORGE}"
  local label="ubuntu-latest:docker://${JOB_IMAGE}"
  case "${FORGE}" in
    gitea)
      cat >"${WORK_DIR}/runner-config.yaml" <<EOF
log:
  level: info
runner:
  file: .runner
  capacity: 1
  labels: ["${label}"]
container:
  network: ${network}
EOF
      ;;
    forgejo)
      cat >"${WORK_DIR}/runner-config.yaml" <<EOF
log:
  level: info
runner:
  capacity: 1
  labels: ["${label}"]
container:
  network: ${network}
server:
  connections:
    e2e:
      url: http://forgejo:3000/
      uuid: ${RUNNER_UUID}
      token: ${RUNNER_SECRET}
EOF
      ;;
  esac
}

seed_action_repo() {
  local ver="$1" bin="$2" src="${WORK_DIR}/src/domarinn"
  api POST /user/repos -d '{"name":"domarinn","private":false}' >/dev/null
  mkdir -p "${src}"
  # The action files exactly as this working tree has them: that is what is
  # under test. Both copies, so a `uses:` typo shows up as a 404 and not as
  # the other forge's action silently running.
  cp -R "${ROOT}/.gitea" "${ROOT}/.forgejo" "${src}/"
  printf '# e2e fixture copy of domarinn\n' >"${src}/README.md"
  fgit -C "${src}" init -q
  fgit -C "${src}" add -A
  fgit -C "${src}" commit -qm "action under test"
  fgit -C "${src}" push -q "${PUSH_BASE}/e2e/domarinn.git" HEAD:main

  # The forge flips a repository from "empty" after a first push in a
  # background queue, and refuses a release (422) until it has.
  local tries=0
  until [ "$(api GET /repos/e2e/domarinn | json_field empty)" = False ]; do
    tries=$((tries + 1))
    [ "${tries}" -lt 60 ] || die "e2e/domarinn still reads as empty after the push"
    sleep 0.5
  done

  # An older release first, with no asset: if `version: latest` resolved to
  # anything but the newest release, the download 404s and the job — whose
  # image has no cargo for the source fallback — fails with exit 3. A
  # second apart, because "latest" is ordered by creation time.
  api POST /repos/e2e/domarinn/releases \
    -d '{"tag_name":"0.0.1","target_commitish":"main","name":"0.0.1"}' >/dev/null
  sleep 1
  local id
  id="$(api POST /repos/e2e/domarinn/releases \
    -d "{\"tag_name\":\"${ver}\",\"target_commitish\":\"main\",\"name\":\"${ver}\"}" | json_field id)"
  # The real asset convention (release.yml): domarinn_<ver>_linux_<arch>.
  curl -fsS -u "${ADMIN_USER}:${ADMIN_PASS}" -X POST \
    -F "attachment=@${bin}" \
    "${BASE}/api/v1/repos/e2e/domarinn/releases/${id}/assets?name=domarinn_${ver}_linux_amd64" >/dev/null
  log "published release ${ver} of e2e/domarinn with asset domarinn_${ver}_linux_amd64"
}

run_forge() {
  FORGE="$1"
  WORK_DIR="${WORK_ROOT}/${FORGE}"
  export FORGE WORK_DIR
  rm -rf "${WORK_DIR}"
  mkdir -p "${WORK_DIR}"
  ADMIN_USER=e2e
  ADMIN_PASS="e2e-$(python3 -c 'import secrets; print(secrets.token_hex(12))')"
  REVIEWER_USER=reviewer
  REVIEWER_PASS="rv-$(python3 -c 'import secrets; print(secrets.token_hex(12))')"
  BASE=""
  trap teardown EXIT

  # Bind-mounted into the runner; must exist as a file before compose does.
  : >"${WORK_DIR}/runner-config.yaml"

  log "${FORGE}: starting the forge"
  compose down -v --remove-orphans >/dev/null 2>&1 || true
  compose up -d --wait "${FORGE}"
  BASE="http://$(compose port "${FORGE}" 3000)"
  PUSH_BASE="http://${ADMIN_USER}:${ADMIN_PASS}@${BASE#http://}"
  log "${FORGE} is up at ${BASE} ($(curl -fsS "${BASE}/api/v1/version" | json_field version))"

  forge_cli admin user create --admin --username "${ADMIN_USER}" --password "${ADMIN_PASS}" \
    --email e2e@example.invalid --must-change-password=false >/dev/null
  forge_cli admin user create --username "${REVIEWER_USER}" --password "${REVIEWER_PASS}" \
    --email reviewer@example.invalid --must-change-password=false >/dev/null

  case "${FORGE}" in
    gitea)
      RUNNER_TOKEN="$(forge_cli actions generate-runner-token | tail -n1)"
      export RUNNER_TOKEN
      ;;
    forgejo)
      # Offline registration: the forge learns a shared secret, the runner
      # config carries it, and no registration token changes hands.
      RUNNER_SECRET="$(python3 -c 'import secrets; print(secrets.token_hex(20))')"
      RUNNER_UUID="$(forge_cli forgejo-cli actions register --name e2e-runner --secret "${RUNNER_SECRET}" | tail -n1)"
      ;;
  esac
  write_runner_config

  log "${FORGE}: starting the runner (docker mode, job image ${JOB_IMAGE})"
  docker pull -q "${JOB_IMAGE}" >/dev/null
  compose up -d "${FORGE}-runner"

  local ver
  ver="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -n1)"
  seed_action_repo "${ver}" "${BINARY}"

  log "${FORGE}: running the pull-request scenario"
  FORGE="${FORGE}" BASE="${BASE}" PUSH_BASE="${PUSH_BASE}" WORK_DIR="${WORK_DIR}" ROOT="${ROOT}" \
    ADMIN_USER="${ADMIN_USER}" ADMIN_PASS="${ADMIN_PASS}" \
    REVIEWER_USER="${REVIEWER_USER}" REVIEWER_PASS="${REVIEWER_PASS}" \
    VERSION="${ver}" RUN_TIMEOUT="${RUN_TIMEOUT:-900}" \
    python3 "${HERE}/scenario.py"

  # Success path: still keep the logs, they are cheap and useful in CI.
  dump_logs
  trap - EXIT
  teardown
}

main() {
  local target="${1:-all}" forges=()
  case "${target}" in
    gitea | forgejo) forges=("${target}") ;;
    all) forges=(gitea forgejo) ;;
    *) die "usage: $0 [gitea|forgejo|all]" ;;
  esac
  command -v docker >/dev/null || die "docker is required"
  docker compose version >/dev/null || die "the docker compose plugin is required"

  mkdir -p "${WORK_ROOT}"
  BINARY="$(resolve_binary)"
  log "publishing $(file -b "${BINARY}" 2>/dev/null | cut -d, -f1-2 || echo "${BINARY}")"

  local results=() failed=0 forge start rc
  for forge in "${forges[@]}"; do
    start=${SECONDS}
    # A subshell per forge, so one forge's failure still runs (and tears
    # down) the next. Not `if ( … )`: bash switches `-e` off inside a
    # command whose status is being tested, and every bootstrap step here
    # relies on it — hence `set +e` out here and `set -e` again inside.
    set +e
    (
      set -e
      run_forge "${forge}"
    )
    rc=$?
    set -e
    if [ "${rc}" -eq 0 ]; then
      results+=("PASS ${forge} ($((SECONDS - start))s)")
    else
      results+=("FAIL ${forge} ($((SECONDS - start))s)")
      failed=1
    fi
  done
  printf '\nforge-e2e results:\n' >&2
  printf '  %s\n' "${results[@]}" >&2
  exit "${failed}"
}

main "$@"
