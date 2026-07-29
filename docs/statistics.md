# Statistics: variance, confidence, and regressions

A single run's pass rate is a point estimate. With LLM-graded evals that vary run to run, "94% passed" without an error bar is not enough to decide whether a change helped. domarinn builds the rigor in.

## Repeat trials for variance

`--repeat N` runs every matrix cell N times. Each trial is a distinct result — the [trial index is part of the cache key](./caching.md#what-is-in-the-key) and of the case identity — so you get a distribution, not a coin flip. That applies to the judge too: N repeats produce N independent verdicts rather than collapsing into one.

```sh
domarinn run --repeat 5
```

Because responses are cached, repeats of a deterministic provider are cheap; for sampling variance, disable caching or vary sampling params. The run summary then counts all trials, and the pass rate is measured over them. To measure *judge* variance specifically, keep provider responses warm and pass `--no-grader-cache`.

## Confidence intervals on the pass rate

The Markdown summary (`run --summary-md summary.md`) reports the pass rate with a 95% **Wilson score interval**:

```
Pass rate: 92.0% (95% CI 84.8%–96.0%, n=50)
```

The Wilson interval is more accurate than the normal approximation for small n and for rates near 0 or 1, and it never leaves `[0, 1]`. **Gate on the lower bound, not the point estimate** — if the CI lower bound clears your bar, the result is defensibly above it.

## Comparing two runs: the diff and `--against`

`domarinn diff <BASE> <HEAD>` (and `run --against <BASE>`) pair the two runs by their stable `case_key` and classify every case:

| Transition | Meaning |
|-----------|---------|
| newly failing | passed in base, fails in head — a **regression** |
| newly passing | failed in base, passes in head — a **fix** |
| still failing | failed in both |
| output changed | same pass/fail, but the output text differs |
| added / removed | present in only one run |

Either command exits `1` when there is at least one newly-failing case, so CI can block a regression. `diff --format md` and `run --summary-md` emit a Markdown table suitable for a PR comment.

## Is the change significant? McNemar's test

Over the cases the two runs share, domarinn runs **McNemar's paired test** with continuity correction. It looks only at the discordant pairs — cases that regressed (`b`) versus cases that were fixed (`c`) — and reports whether the difference is significant at the 95% level (statistic > 3.841, 1 df):

```
McNemar: 12 regressions vs 3 fixes, statistic 4.27 (significant at 95%)
```

This distinguishes "20 regressions and 2 fixes" (a real shift) from "5 and 4" (noise), which raw counts cannot.

## pass@k

For repeated trials, domarinn computes the unbiased **pass@k** estimator — the probability that at least one of `k` sampled trials passes — the standard metric for "does it work if we retry." Use `--repeat N` to gather the trials it is computed over.

## Baselines on the server

The server can mark a run as the **baseline** for a project/suite, and the compare view defaults to it. This gives a stable reference to gate against across CI runs without threading run ids by hand. See [server.md](./reference/server.md).

## A CI-ready pattern

```sh
domarinn run \
  --repeat 5 \
  --against latest \
  --format junit --out results.xml \
  --summary-md summary.md
# exit 1 => a regression (block the PR); 3 => the harness broke (retry)
```

The reusable GitHub Action wraps exactly this and posts the summary as a PR comment — see [ci.md](./ci.md).

## pass@1 and repeats

The stats footer reports pass@1 whenever a run had repeats. It differs from the plain pass rate exactly when a case's trials disagreed — which is the model or judge variance `--repeat` exists to measure, and which averaging into a single pass rate hides. With `--repeat 1` the two are identical, so it is omitted rather than printed twice under two names.
