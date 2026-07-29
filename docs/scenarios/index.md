# Scenarios

The [Examples](../examples.md) page is a ladder of **per-capability** suites — one feature per file. **Scenarios** are the layer above: end-to-end walkthroughs for a real situation, tying several capabilities, the right commands, and the verification steps together.

Each one is built from suites on the Examples page, so everything here is runnable and everything here is tested.

/// tip | Read this first if you are new

domarinn evaluates a **grid**: every provider against every prompt against every test, one call per cell. Assertions grade the answer; cheap ones run first and can short-circuit the expensive ones; a cell ends `pass`, `fail`, `error` or `skip`.

The load-bearing distinction in most of these scenarios is **`fail` versus `error`**. A failure means the system under test got worse. An error means you learned nothing. They gate differently — exit `1` and exit `3` — and a pipeline that conflates them is a pipeline people stop reading.

See [How a run works](../concepts/how-a-run-works.md).

///

| # | Scenario | When you reach for it | Cost |
| - | -------- | --------------------- | ---- |
| 01 | [A zero-cost gate on every PR](render-gate.md) | Catch template and formatting regressions with no key and no spend. | Free |
| 02 | [Grade an assistant against a policy](grade-against-policy.md) | Scope enforcement, refusals, and rubrics that grade one thing. | Judge only |
| 03 | [Evaluate structured output](structured-output.md) | An agent that must emit a parseable object with correct field semantics. | Judge only |
| 04 | [Gate a pull request on regressions](regression-gate.md) | Block a merge that makes things worse, without demanding perfection. | Per run |
| 05 | [Share a cache across a team](shared-cache.md) | Stop everyone paying separately for the same answers. | Saves money |
| 06 | [Evaluate your own application](evaluate-your-app.md) | Test what actually ships, not the model underneath it. | Depends |

## The two-layer pattern

Most teams that get value out of this end up with the same shape, and it is worth stating plainly because it is not obvious from the reference docs:

- **A deterministic layer** that runs on *every* pull request. No API key, no spend, seconds to run. It catches render holes, missing sections, format drift, leaked internals. [Scenario 01](render-gate.md).
- **A graded layer** that runs on a schedule or on demand, costs real money, and is gated against a stored baseline. It catches behavioural regressions the first layer cannot see. [Scenario 04](regression-gate.md).

Trying to do both in one suite means either paying for the cheap checks or skipping the expensive ones. Splitting them means the fast gate can be mandatory.

## See also

- [Examples](../examples.md) — the per-capability ladder these are built from.
- [Getting started](../getting-started.md) — install and first run.
- [Troubleshooting](../troubleshooting.md) — when a step does not go green.
