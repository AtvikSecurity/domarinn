# Caching & statistics

Four suites about not paying twice for the same answer, and about trusting a pass rate once you do pay for one. They cover the single rule the cache key follows, busting it at the right granularity, turning one run into a confidence interval, and gating on a regression rather than an absolute score. Read these once a suite is calling a real model and cost or confidence starts to matter.

---

## Example 21 — Caching

Every outgoing request is cached, content-addressed, on by default. Run this suite twice and the second run does no work at all.

```yaml
--8<-- "examples/21-caching-basics/domarinn.yaml"
```

The rule the key follows is one sentence: **hash what is sent.** A provider call, the LLM judge, an embedding and an `exec` grader are all keyed the same way — the SHA-256 of the redacted request, plus the trial index, plus any `cache_salt` in scope. Nothing about your machine, your binary, or your credentials.

That is what makes a cache shareable. A key that varied by machine could not be reused by anyone else, which quietly turns a shared cache into an expensive local disk.

Three consequences worth knowing:

- **One entry per key, immutable.** The first write wins, on every backend.
- **Errors are never cached.** Only successful responses are stored.
- **`latency` assertions bypass the cache entirely**, because a replayed response has no honest latency to report — and under `--cache-only` such a case is refused rather than called live. `cost` and `tokens` come from the stored response.

Note the `cache:` block names only the *kind* of backend. The URL and credentials come from the environment, so a suite stays safe to commit. See [Caching](../concepts/caching.md#the-rule) for the full rule and the shared backends.

---

## Example 22 — Cache salts

The key is the request, and a request only carries what domarinn can **see**. When the system under test loads its own content across a process boundary — prompts from a registry, rules from a database — domarinn never sees that content, so editing it changes nothing about the request and the cache keeps answering with yesterday's results.

```yaml
--8<-- "examples/22-cache-salts/domarinn.yaml"
```

`cache_salt` is the lever, and it exists at two levels because it is really two problems:

| Level | What it is | Bump it when |
| ----- | ---------- | ------------ |
| **Provider** | A coarse "same build?" version pin. | The program's own logic changes. |
| **Per case** | A content digest of just what *this* case exercises. | Never by hand — `$digest:` computes it. |

Do **not** make the provider-level salt a content digest of everything the program reads. That throws the whole cache away on any edit, which is precisely the outcome the per-case salt exists to avoid. The two-level arrangement is what keeps a large suite affordable while staying honest: edit one prompt and only the cases that use it re-run.

`$digest:` renders its glob against the case's own vars, hashes matched files in sorted order *with their relative paths*, and treats a glob matching nothing as an error — because an empty digest would silently mean "never bust".

---

## Example 23 — Repeat and confidence

A pass rate off one run of twenty cases is a number with no error bar. Models are stochastic, and so is anything built on them: "17/20 passed" and "17/20 passed, 95% CI [0.62, 0.96]" are the same measurement, but only one tells you whether yesterday's 15/20 was a regression or noise.

```yaml
--8<-- "examples/23-repeat-and-confidence/domarinn.yaml"
```

`--repeat N` runs every cell N times, and the report gains three things:

- **Wilson confidence intervals** on the pass rate — well-behaved at small N and at rates near 0 or 1, where the normal approximation is simply wrong.
- **pass@k** — did at least one of k attempts succeed, which is the right question for anything with a retry loop in front of it.
- **McNemar significance** when diffing two runs — a *paired* test, because both runs saw the same cases, and treating them as independent samples throws away exactly the information that makes the comparison sharp.

The trial index is part of the cache key, so repeats genuinely re-sample instead of replaying one cached answer N times.

---

## Example 24 — Baselines and diff

A pass rate on its own cannot tell you whether a change made things worse. `--against` compares a run to a baseline cell by cell and gates on **regressions** — cases that passed before and fail now — so a suite that was 80% green yesterday does not have to be 100% green today to merge.

```yaml
--8<-- "examples/24-baselines-and-diff/domarinn.yaml"
```

/// danger | `--against latest` silently never gates in CI

`latest` resolves through a **cwd-relative** `.domarinn/runs/latest`. A fresh CI checkout has no such directory, so it finds nothing, logs a *warning*, and lets the job exit `0` on a real regression.

It is right for local iteration and useless in CI. Use `--against server:baseline` there — and note that "no baseline pinned" is *also* only a warning, so the gate is never better than what is actually pinned. A stale or partial baseline compares almost nothing and reads as a pass.

///

Baselines are keyed per provider id, so renaming a provider — or changing the model inside its `command` — starts its history over.
