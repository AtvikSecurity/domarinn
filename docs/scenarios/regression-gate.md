# Gate a pull request on regressions

**The problem.** A behavioural suite is never 100% green. Gating on an absolute pass rate means either setting the bar so low it catches nothing, or blocking every merge. What you actually want to block is a change that made things **worse**.

**The shape.** Compare this run to a stored baseline, cell by cell, and fail on cases that passed before and fail now.

## 1. Store runs somewhere CI can reach

Regression gating needs a baseline that survives a fresh checkout. Run the [results server](../reference/server.md) and point runs at it:

```console
$ export DOMARINN_SERVER_URL=https://evals.example.com
$ export DOMARINN_TOKEN=...
$ domarinn run eval/behavioral.yaml --share
```

Then pin a known-good run as the baseline for that project/suite in the web UI.

## 2. Gate against it

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

## 3. Know exactly what the gate does and does not catch

/// danger | Three ways this gate silently passes

**`--against latest` in CI.** It resolves through a cwd-relative `.domarinn/runs/latest`, which a fresh checkout does not have. It finds nothing, warns, and exits `0` on a real regression. Use `server:baseline`.

**No baseline pinned.** Also a warning, not a failure. The gate is only ever as good as what is actually pinned — and a stale or one-case baseline compares almost nothing while reading as a pass. Re-pin it after any deliberate improvement, and check what it contains.

**A version mismatch.** The action shells out to `domarinn ci-summary`. Against an older binary that subcommand does not exist, and the step degrades to a stub comment on a green run. Pin the action and `DOMARINN_VERSION` as a pair.

///

Also note baselines are keyed **per provider id**. Renaming a provider — or changing the model inside its `command` — starts its history over, silently.

## 4. Read the exit code correctly

| Code | Meaning | What to do |
| ---- | ------- | ---------- |
| `0` | Everything passed. | Merge. |
| `1` | An assertion failed. | The system under test got worse. Look at the diff. |
| `2` | Usage error. | The suite or the flags are malformed. |
| `3` | Infrastructure error. | The harness broke. **Do not** read this as a quality signal. |

`3` beats `1` when both occur, so a run that both regressed and broke reports as broken — which is the honest answer, because the regression may be an artefact of the breakage.

## 5. Report it where people look

```console
$ domarinn ci-summary latest --out summary.md --github-output "$GITHUB_OUTPUT"
```

`ci-summary` is a **reporter, not a gate** — it always exits `0`. The gate is the exit code of `run`. Keeping them separate means a reporting failure never blocks a merge and never fakes one.

## See also

- [Example 24](../examples/caching-and-statistics.md#example-24--baselines-and-diff) — the suite above.
- [CI integration](../ci.md) — the action's inputs and outputs.
- [Statistics](../statistics.md) — McNemar significance on a paired diff.
