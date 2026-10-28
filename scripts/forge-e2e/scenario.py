#!/usr/bin/env python3
"""The pull-request scenario of the forge e2e harness (run.sh sets it up).

Stdlib only. Everything is asserted through the forge's REST API, as the
admin `e2e` user (who opens the pull request) and a second, unprivileged
`reviewer` user. The comment under test is written by the job's automatic
token, so it is the one comment authored by neither of them.

  1. Open a PR; the reviewer immediately plants a comment that STARTS with the
     action's marker. The run passes; exactly one comment is the action's,
     starts with the marker and carries the summary; the planted one is
     untouched (the author check, not the marker, decides what is edited).
  2. The reviewer quote-replies the action's comment (the marker, not at the
     start). A second commit's run UPDATES the action's comment in place:
     same id, new content, still exactly one; both reviewer comments intact.
  3. A third commit makes one case fail. The job concludes failure (exit 1
     with fail-on-regression) and the same comment says so.
  Every run: the action's outputs (read from the job log), `version: latest`
  resolved through the forge's release API, the results artifact and its two
  files, no comment-step warnings, and the `CI run` link in the summary opens
  this run's page on the forge. A warm-up run in another repository comes
  first, so run ids and run numbers differ and a link built from the wrong
  one lands on the wrong page.
"""

import base64
import io
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

FORGE = os.environ["FORGE"]
BASE = os.environ["BASE"].rstrip("/")
PUSH_BASE = os.environ["PUSH_BASE"].rstrip("/")
WORK = Path(os.environ["WORK_DIR"])
ROOT = Path(os.environ["ROOT"])
ADMIN = (os.environ["ADMIN_USER"], os.environ["ADMIN_PASS"])
REVIEWER = (os.environ["REVIEWER_USER"], os.environ["REVIEWER_PASS"])
RUN_TIMEOUT = int(os.environ.get("RUN_TIMEOUT", "900"))
HERE = Path(__file__).resolve().parent

# What the jobs see: the ROOT_URL, which is the forge's name on the compose
# network. The host reaches the same server on a mapped loopback port.
INTERNAL = f"http://{FORGE}:3000"
MARKER = "<!-- domarinn-eval -->"
REPO = "e2e/consumer"
LOGS = WORK / "logs"


class Failed(Exception):
    pass


class ToHost(urllib.request.HTTPRedirectHandler):
    """Follow a redirect to the forge's ROOT_URL on the host's port instead.

    The forge builds absolute redirects from its ROOT_URL, the compose-network
    name the jobs use, which does not resolve on the host. Rewriting it is
    exactly the substitution the host side makes everywhere else.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if newurl.startswith(INTERNAL):
            newurl = BASE + newurl[len(INTERNAL):]
        return super().redirect_request(req, fp, code, msg, headers, newurl)


OPENER = urllib.request.build_opener(ToHost)


def step(msg):
    print(f"\n--- [{FORGE}] {msg}", flush=True)


def check(cond, msg):
    if not cond:
        raise Failed(msg)
    print(f"  ok: {msg}", flush=True)


def request(method, path, body=None, auth=ADMIN, raw=False, url=None):
    target = url or f"{BASE}/api/v1{path}"
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(target, data=data, method=method)
    if auth is not None:
        token = base64.b64encode(f"{auth[0]}:{auth[1]}".encode()).decode()
        req.add_header("Authorization", f"Basic {token}")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "application/json")
    try:
        with OPENER.open(req, timeout=60) as res:
            text = res.read().decode("utf-8", "replace")
            if raw:
                return res.status, text
            return json.loads(text) if text else None
    except urllib.error.HTTPError as e:
        if raw:
            return e.code, e.read().decode("utf-8", "replace")
        raise Failed(f"{method} {target} -> HTTP {e.code}: {e.read()[:300]!r}") from None


def fetch(url, auth=None):
    """GET raw bytes, following redirects; (status, body)."""
    req = urllib.request.Request(url)
    if auth is not None:
        token = base64.b64encode(f"{auth[0]}:{auth[1]}".encode()).decode()
        req.add_header("Authorization", f"Basic {token}")
    try:
        with OPENER.open(req, timeout=60) as res:
            return res.status, res.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def git(*args, cwd):
    env = dict(
        os.environ,
        GIT_CONFIG_GLOBAL="/dev/null",
        GIT_CONFIG_NOSYSTEM="1",
        GIT_AUTHOR_NAME="e2e",
        GIT_AUTHOR_EMAIL="e2e@example.invalid",
        GIT_COMMITTER_NAME="e2e",
        GIT_COMMITTER_EMAIL="e2e@example.invalid",
    )
    out = subprocess.run(
        ["git", "-c", "init.defaultBranch=main", "-c", "core.hooksPath=/dev/null", *args],
        cwd=cwd, env=env, check=True, capture_output=True, text=True,
    )
    return out.stdout.strip()


# --- Fixture --------------------------------------------------------------


def suite(variant):
    """The suite at each commit. Exec provider = the shipped echo provider."""
    cases = [
        ("greeting/basic", "hello world", "contains", "hello"),
        ("ssti/literal", "{{7*7}}", "not-contains", "49"),
    ]
    if variant >= 2:
        cases.append(("echo/new-case", "a new case", "contains", "new case"))
    if variant >= 3:
        # The echo provider answers with the input, so this cannot pass.
        cases[-1] = ("echo/new-case", "a new case", "contains", "something else")
    lines = [
        "version: 1",
        "project: e2e",
        "suite: forge-e2e",
        "providers:",
        "  - id: echo",
        "    type: exec",
        '    command: ["python3", "../echo-provider.py"]',
        "    timeout_ms: 30000",
        "tests:",
    ]
    for cid, user_input, kind, value in cases:
        lines += [
            f"  - id: {cid}",
            "    vars:",
            f"      user_input: !raw {json.dumps(user_input)}",
            "    assert:",
            f"      - type: {kind}",
            f"        value: {json.dumps(value)}",
        ]
    return "\n".join(lines) + "\n"


def workflow():
    text = (HERE / "fixture" / "eval.yml").read_text()
    if FORGE == "gitea":
        # Gitea 1.26+ scopes the automatic token by this block (the guide's
        # Gitea example); Forgejo documents no such key.
        perms = "permissions:\n  contents: read\n  issues: write\n  pull-requests: write"
        checkout = "actions/checkout@v4"
    else:
        perms = "# Forgejo documents no `permissions:` key."
        checkout = "https://data.forgejo.org/actions/checkout@v4"
    return (
        text.replace("# @PERMISSIONS@", perms)
        .replace("@CHECKOUT@", checkout)
        .replace("@FORGE_URL@", INTERNAL)
        .replace("@FORGE@", FORGE)
    )


def push(src, repo, branch):
    """Push HEAD and wait until the forge has caught up with it.

    Both forges process a push in a background queue: until it lands, a new
    repository still reads as empty and a new branch does not exist, and a
    release or a pull request against it is refused (422/404).
    """
    git("push", "-q", f"{PUSH_BASE}/{repo}.git", f"HEAD:{branch}", cwd=src)
    sha = git("rev-parse", "HEAD", cwd=src)
    for _ in range(120):
        status, text = request("GET", f"/repos/{repo}/branches/{branch}", raw=True)
        if status == 200 and json.loads(text)["commit"]["id"] == sha:
            return sha
        time.sleep(0.5)
    raise Failed(f"{repo}@{branch} never showed {sha[:10]} after the push")


def commit_suite(src, variant, message):
    (src / "eval" / "suite" / "domarinn.yaml").write_text(suite(variant))
    git("add", "-A", cwd=src)
    git("commit", "-qm", message, cwd=src)
    return push(src, REPO, "change")


def warm_up():
    """One run in another repository before any of the consumer's.

    It moves the instance-wide run ids off the per-repository run numbers
    (the consumer's first run becomes id 2, number 1), which is the only
    situation in which a CI link built from the wrong one of the two opens the
    wrong page — so it is what makes the link check below mean anything. It
    also pulls the job image and the checkout action once, up front.
    """
    step("warm-up run in e2e/warmup (separates run ids from run numbers)")
    src = WORK / "src" / "warmup"
    if src.exists():
        shutil.rmtree(src)
    (src / f".{FORGE}" / "workflows").mkdir(parents=True)
    request("POST", "/user/repos", {"name": "warmup", "private": False})
    (src / f".{FORGE}" / "workflows" / "warm.yml").write_text(
        "on: push\njobs:\n  warm:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo warm\n"
    )
    git("init", "-q", cwd=src)
    git("add", "-A", cwd=src)
    git("commit", "-qm", "warm up", cwd=src)
    sha = push(src, "e2e/warmup", "main")
    run, _ = wait_run(sha, "warmup", repo="e2e/warmup")
    check(run_conclusion(run) == "success", "the warm-up run succeeded")


# --- Runs -----------------------------------------------------------------


def find_run(sha, repo=REPO):
    runs = request("GET", f"/repos/{repo}/actions/runs?limit=50")
    runs = runs.get("workflow_runs", runs) if isinstance(runs, dict) else runs
    for run in runs or []:
        if sha in (run.get("head_sha"), run.get("commit_sha")):
            return run
    return None


def run_done(run):
    status = (run.get("status") or "").lower()
    return status in {"completed", "success", "failure", "cancelled", "skipped"}


def run_conclusion(run):
    return (run.get("conclusion") or run.get("status") or "").lower()


def jobs_of(run, repo=REPO):
    # By the run's API id — on Forgejo too, where the web UI's run number
    # would name a different run (or none) here.
    jobs = request("GET", f"/repos/{repo}/actions/runs/{run['id']}/jobs")
    return jobs.get("jobs", jobs) if isinstance(jobs, dict) else jobs


def job_log(run, repo=REPO):
    out = []
    for job in jobs_of(run, repo) or []:
        status, text = request("GET", f"/repos/{repo}/actions/jobs/{job['id']}/logs", raw=True)
        out.append(text if status == 200 else f"<log for job {job['id']}: HTTP {status}>")
    return "\n".join(out)


def wait_run(sha, label, repo=REPO):
    step(f"waiting for the run of {sha[:10]} ({label})")
    deadline = time.time() + RUN_TIMEOUT
    run = None
    while time.time() < deadline:
        run = find_run(sha, repo)
        if run is not None and run_done(run):
            break
        time.sleep(3)
    else:
        LOGS.mkdir(parents=True, exist_ok=True)
        (LOGS / f"timeout-{label}.json").write_text(json.dumps(run, indent=2))
        if run is not None:
            (LOGS / f"timeout-{label}-job.log").write_text(job_log(run, repo))
        raise Failed(f"run for {sha[:10]} did not complete in {RUN_TIMEOUT}s (last: {run and run.get('status')})")
    log = job_log(run, repo)
    LOGS.mkdir(parents=True, exist_ok=True)
    (LOGS / f"job-{label}.log").write_text(log)
    log = output_only(log)
    print(f"  run id={run['id']} number={run.get('run_number') or run.get('index_in_repo')} -> {run_conclusion(run)}", flush=True)
    return run, log


LOG_PREFIX = re.compile(r"^\d{4}-\d\d-\d\dT[\d:.]+Z ?")


def output_only(log):
    """The job log without the runner's echo of each `run:` script.

    Both runners print a step's script inside `::group::Run …`/`::endgroup::`
    before running it, and the action's scripts contain the very warning texts
    asserted absent below. Everything outside those groups is real output.
    """
    kept, echo = [], False
    for line in log.splitlines():
        text = LOG_PREFIX.sub("", line)
        if text.startswith("::group::Run "):
            echo = True
        elif echo and text.startswith("::endgroup::"):
            echo = False
        elif not echo:
            kept.append(text)
    return "\n".join(kept)


def outputs_from(log):
    found = re.findall(r"^E2E-OUTPUTS (.*)$", log, re.M)
    if not found:
        raise Failed("the job log has no E2E-OUTPUTS line (did the Report outputs step run?)")
    return dict(kv.split("=", 1) for kv in found[-1].split())


def comments(pr):
    return request("GET", f"/repos/{REPO}/issues/{pr}/comments")


def bot_comments(all_comments):
    people = {ADMIN[0], REVIEWER[0]}
    return [c for c in all_comments if c["user"]["login"] not in people]


def assert_common(run, log, label):
    """What every run must show, pass or fail."""
    # The comment step reports its problems as warnings and never fails the
    # job, so a broken comment path is only visible here.
    warnings = [line for line in log.splitlines() if line.startswith("::warning::")]
    for w in warnings:
        print(f"  note: job warning: {w}")
    for needle in ("comment", "token's user", "summarize", "building domarinn from source"):
        bad = [w for w in warnings if needle in w]
        check(not bad, f"{label}: no '{needle}' warning from the action")
    check(re.search(r"^(Posted|Updated) PR comment \d+$", log, re.M) is not None,
          f"{label}: the comment step reports a posted or updated comment")
    check("Resolved latest to " + os.environ["VERSION"] in log,
          f"{label}: `version: latest` resolved to {os.environ['VERSION']} through the forge release API")

    assert_artifact(run, label)


def assert_artifact(run, label):
    """The upload step's artifact exists and holds what the action promises.

    The artifacts API first. Gitea's lists only v4 artifacts, and its action
    copy uploads with upload-artifact v3 (see that file's header), so there the
    API is empty by design; the run page's own download route serves either
    protocol, and its zip is checked for both files.
    """
    arts = request("GET", f"/repos/{REPO}/actions/runs/{run['id']}/artifacts")
    arts = arts.get("artifacts", []) if isinstance(arts, dict) else arts or []
    print(f"  artifacts API lists: {[a.get('name') for a in arts]}")
    ours = [a for a in arts if a.get("name") == "domarinn-results"]
    if FORGE == "gitea":
        # The run page's download route (the run's own html_url, so it is
        # addressed however this Gitea addresses runs).
        url = f"{run['html_url']}/artifacts/domarinn-results"
        url = BASE + url[url.index(f"/{REPO}/"):]
        auth = None
    else:
        check(len(ours) == 1, f"{label}: the artifacts API lists domarinn-results")
        url = ours[0]["archive_download_url"]
        url = BASE + url[url.index("/api/v1/"):]
        auth = ADMIN
    status, blob = fetch(url, auth)
    if status != 200:
        raise Failed(f"{label}: artifact download {url} -> HTTP {status}")
    with zipfile.ZipFile(io.BytesIO(blob)) as zf:
        files = sorted(zf.namelist())
    check(files == ["results.xml", "summary.md"],
          f"{label}: the domarinn-results artifact downloads with results.xml and summary.md ({files})")


def assert_ci_link(run, body, label):
    m = re.search(r"\[CI run\]\(([^)]+)\)", body)
    check(m is not None, f"{label}: the summary links the CI run")
    url = m.group(1)
    number = run.get("run_number") or run.get("index_in_repo")
    check(str(number) != str(run["id"]),
          f"precondition: run id {run['id']} differs from run number {number} (see warm_up)")
    path = f"/{REPO}/actions/runs/{number}"
    check(url.startswith(INTERNAL + path), f"{label}: CI run link {url} names run number {number}")
    status, page = request("GET", "", raw=True, auth=None, url=BASE + url[len(INTERNAL):])
    check(status == 200, f"{label}: CI run link answers 200 on {FORGE} (got {status})")
    # 200 is not enough: on the wrong side of the id/number split the link
    # opens a different run of this repository. Each commit here has its own
    # title, and the run page shows it.
    title = run.get("display_title") or run.get("title")
    check(bool(title) and title in page, f"{label}: the linked page is this run's ({title!r})")


def main():
    LOGS.mkdir(parents=True, exist_ok=True)
    warm_up()

    src = WORK / "src" / "consumer"
    if src.exists():
        shutil.rmtree(src)
    (src / "eval" / "suite").mkdir(parents=True)
    (src / f".{FORGE}" / "workflows").mkdir(parents=True)

    step("seeding e2e/consumer")
    request("POST", "/user/repos", {"name": "consumer", "private": False})
    (src / f".{FORGE}" / "workflows" / "eval.yml").write_text(workflow())
    shutil.copy(ROOT / "examples" / "echo-provider.py", src / "eval" / "echo-provider.py")
    (src / "eval" / "suite" / "domarinn.yaml").write_text(suite(1))
    (src / ".gitignore").write_text(".domarinn/\n")
    git("init", "-q", cwd=src)
    git("add", "-A", cwd=src)
    git("commit", "-qm", "eval suite", cwd=src)
    push(src, REPO, "main")
    git("checkout", "-qb", "change", cwd=src)
    (src / "README.md").write_text("change under review\n")
    git("add", "-A", cwd=src)
    git("commit", "-qm", "first change", cwd=src)
    sha1 = push(src, REPO, "change")

    # -- 1 ------------------------------------------------------------------
    step("scenario 1: open the PR; a reviewer plants a marker-first comment")
    pr = request("POST", f"/repos/{REPO}/pulls",
                 {"title": "e2e change", "head": "change", "base": "main"})["number"]
    planted_body = f"{MARKER}\nplanted by the reviewer: the action must never edit this"
    planted = request("POST", f"/repos/{REPO}/issues/{pr}/comments", {"body": planted_body}, auth=REVIEWER)

    run1, log1 = wait_run(sha1, "run1")
    check(run_conclusion(run1) == "success", "run 1 concluded success")
    out = outputs_from(log1)
    check(out.get("exit-code") == "0", f"run 1 exit-code output is 0 ({out})")
    check(out.get("passed") == "2" and out.get("total") == "2" and out.get("failed") == "0",
          "run 1 outputs passed=2 failed=0 total=2")
    assert_common(run1, log1, "run 1")

    all1 = comments(pr)
    bots = bot_comments(all1)
    check(len(bots) == 1, f"exactly one comment from the action after run 1 (found {len(bots)})")
    bot = bots[0]
    print(f"  the action commented as {bot['user']['login']!r} (id {bot['user']['id']})")
    check(bot["body"].startswith(MARKER), "the action's comment starts with the marker")
    check("2 passed" in bot["body"], "the action's comment carries the summary (2 passed)")
    check(bot["id"] > planted["id"],
          "precondition: the planted comment is older, so a marker-only match would pick it")
    now_planted = next(c for c in all1 if c["id"] == planted["id"])
    check(now_planted["body"] == planted_body, "the reviewer's marker-first comment is untouched")
    assert_ci_link(run1, bot["body"], "run 1")

    # -- 2 ------------------------------------------------------------------
    step("scenario 2: reviewer quote-replies; a second commit updates in place")
    quote_body = "\n".join("> " + line for line in bot["body"].splitlines()) + "\n\nLooks right to me."
    check(MARKER in quote_body and not quote_body.startswith(MARKER),
          "precondition: the quote-reply carries the marker, not at the start")
    quote = request("POST", f"/repos/{REPO}/issues/{pr}/comments", {"body": quote_body}, auth=REVIEWER)
    sha2 = commit_suite(src, 2, "add a case")
    run2, log2 = wait_run(sha2, "run2")
    check(run_conclusion(run2) == "success", "run 2 concluded success")
    out = outputs_from(log2)
    check(out.get("exit-code") == "0" and out.get("passed") == "3" and out.get("total") == "3",
          f"run 2 outputs exit-code=0 passed=3 total=3 ({out})")
    assert_common(run2, log2, "run 2")
    all2 = comments(pr)
    bots2 = bot_comments(all2)
    check(len(bots2) == 1, f"still exactly one comment from the action (found {len(bots2)})")
    check(bots2[0]["id"] == bot["id"], "it is the same comment, updated in place")
    check(bots2[0]["body"] != bot["body"] and "3 passed" in bots2[0]["body"],
          "its content changed to the new summary (3 passed)")
    by_id = {c["id"]: c for c in all2}
    check(by_id[planted["id"]]["body"] == planted_body, "the planted comment is still untouched")
    check(by_id[quote["id"]]["body"] == quote_body, "the reviewer's quote-reply is untouched")
    assert_ci_link(run2, bots2[0]["body"], "run 2")

    # -- 3 ------------------------------------------------------------------
    step("scenario 3: a failing case fails the job and the comment says so")
    sha3 = commit_suite(src, 3, "break a case")
    run3, log3 = wait_run(sha3, "run3")
    check(run_conclusion(run3) == "failure", f"run 3 concluded failure (got {run_conclusion(run3)})")
    out = outputs_from(log3)
    check(out.get("exit-code") == "1" and out.get("failed") == "1" and out.get("passed") == "2",
          f"run 3 outputs exit-code=1 passed=2 failed=1 ({out})")
    check("::error::domarinn reported assertion failures or regressions (exit 1)" in log3,
          "the gate step failed the job on exit 1")
    assert_common(run3, log3, "run 3")
    all3 = comments(pr)
    bots3 = bot_comments(all3)
    check(len(bots3) == 1 and bots3[0]["id"] == bot["id"],
          "still the one comment from the action, same id")
    check("1 failed" in bots3[0]["body"] and bots3[0]["body"] != bots2[0]["body"],
          "the comment now reports the failure")
    by_id = {c["id"]: c for c in all3}
    check(by_id[planted["id"]]["body"] == planted_body and by_id[quote["id"]]["body"] == quote_body,
          "both reviewer comments are still untouched")

    print(f"\n[{FORGE}] all scenarios passed", flush=True)


if __name__ == "__main__":
    try:
        main()
    except Failed as e:
        print(f"\n[{FORGE}] FAILED: {e}", file=sys.stderr, flush=True)
        sys.exit(1)
