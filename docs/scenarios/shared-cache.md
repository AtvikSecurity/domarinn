# Share a cache across a team

**The problem.** Five engineers and CI all run the same behavioural suite. Every one of them pays separately for identical answers, and a full run is slow enough that people stop doing it before merging.

**The shape.** A content-addressed cache with a shared tier behind the local disk. The first person to ask a question pays; everyone else replays.

## Why this works at all

domarinn's cache key contains **nothing about your machine** — no path, no mtime, no digest of the program's own bytes. Just the request, and a verbatim name for whatever receives it.

That is the whole reason a shared cache is possible. A key that varied by machine would produce a per-developer keyspace with a shared bucket behind it: all the operational cost of a distributed cache, none of the hits.

```yaml
--8<-- "examples/21-caching-basics/domarinn.yaml"
```

## 1. Pick a backend

| `backend` | Shared tier | Use when |
| --------- | ----------- | -------- |
| `disk` | none | Solo work. The default. |
| `http` | the results server's `/api/v1/cache` | You already run the server. Simplest. |
| `s3` | any S3-compatible bucket (MinIO, Garage, SeaweedFS) | You have object storage and no server. |
| `layered` | local disk in front of a remote | Almost always what you want in practice. |

Every remote keeps the **local tier in front**, so a warm local hit never touches the network.

/// tip | The config names only the kind

```yaml
cache:
  backend: layered
```

No URL, no credentials — those come from the environment (`DOMARINN_SERVER_URL`, `DOMARINN_TOKEN`, or the AWS credential chain). That is what makes a suite safe to commit, and it means the same file works locally and in CI with no branching.

If the credentials are missing, domarinn **falls back to local disk with a warning** rather than failing the run. Convenient, and worth knowing about: a misconfigured CI job will look like it is working while paying full price. Check for the warning.

///

## 2. Salt at the right granularity

This is the part that decides whether a shared cache stays useful or gets thrown away weekly.

```yaml
--8<-- "examples/22-cache-salts/domarinn.yaml"
```

Two levels, two different jobs:

- **Provider-level `cache_salt`** — a coarse "same build?" pin. Bump it when the program's own logic changes. A commit SHA or a release tag.
- **Per-case `cache_salt: "$digest: …"`** — a content digest of just what *this* case exercises.

Do **not** make the provider-level salt a content digest of everything your program reads. It works, and it discards the entire cache on any edit — which is exactly the outcome the per-case salt exists to prevent. With both in place, editing one prompt re-runs the handful of cases that use it and replays the rest.

## 3. Know what is and is not cached

- **One entry per key, immutable.** First write wins, on every backend — so concurrent writers are race-free by construction.
- **Errors are never cached.** Only successful responses.
- **Grader verdicts are cached too**, keyed on the graded output, so busting a response busts its verdict in lockstep. Disable with `cache.grader: false` while iterating on a rubric.
- **A `threshold` is not in the verdict key.** The cached value is the raw verdict and the threshold is applied on read — so editing a threshold re-scores instantly instead of re-paying the judge.
- **Pricing is not in the key either.** `cost_usd` is recomputed on every hit, so correcting a rate re-prices history rather than discarding it.
- **`latency` assertions bypass the cache entirely.** A replayed response has no honest latency.

## 4. Verify it is actually shared

```console
$ domarinn run eval/behavioral.yaml
$ domarinn run eval/behavioral.yaml        # second run
```

The second run should report every cell as a cache hit. If it does not, something in the suite varies between runs — a `now()` in a var, a `$digest:` glob matching a file that is being rewritten, or a salt containing a timestamp.

```console
$ domarinn cache stats eval/
$ domarinn cache gc --older-than 30d eval/
```

## See also

- [Caching](../caching.md) — the full key semantics and backend details.
- [Example 21](../examples.md#example-21--caching) and [22](../examples.md#example-22--cache-salts).
- [Server](../server.md) — running the shared tier.
