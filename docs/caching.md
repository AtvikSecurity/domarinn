# Caching

domarinn caches provider responses so re-running a suite is fast, cheap, and deterministic — and so a team can share that work.

## The rule

**Hash what is sent. Name what receives it.**

- `command`, `env`, `model`, `base_url`, `url` and `headers` **select** a provider. They enter the key verbatim, exactly as written in the suite.
- The rendered prompt, the vars, the tools, and a grader `template`'s bytes **are** the request. They are hashed.
- Nothing else — and in particular no property of the local filesystem. No path, no `mtime`, no size, no digest of the program's own bytes.

That last clause is what makes a cache shareable. A key that varies by machine cannot be reused by anyone else, which quietly turns the S3 and results-server backends into an expensive local disk. Because the key is a pure function of the suite text and the rendered request, the same question keys the same way in every checkout, on every machine, from any working directory.

The rule cuts in a place that can look arbitrary until you apply it twice. A grader `template`'s **contents** are hashed, because those bytes get pasted into the prompt the judge reads — they are part of the question. An `exec` program's contents are not, because that program is what *receives* the question. Same file, same digest algorithm, opposite answers, one rule.

## How the cache works

The cache is **content-addressed**. The key is a SHA-256 of the provider's fingerprint plus the rendered request, the case's [`cache_salt`](#per-case-salts) when it sets one, and the repeat index. Same input, same key.

- **One entry per key, immutable.** The first write wins, on every backend. Two writers who agree on a key agree on the *answer* — though not on the bytes, since an entry also records when the call happened, how many attempts it took, and which version wrote it.
- **Errors are never cached.** Only successful responses are stored.
- **Grader verdicts are cached too**, on by default like everything else. An LLM-graded suite used to re-pay its judge on every run even when every provider response was a cache hit, which is the dominant recurring cost of running one. Disable with `cache.grader: false` or `--no-grader-cache`.
- **A `threshold` is not in the verdict key.** The cached value is the raw verdict, and the threshold is applied on read — so editing a `threshold:` re-scores every case instantly instead of re-paying the judge for an answer it already gave. This is structural: the cached type has no threshold in it.
- **Neither is pricing.** `cost_usd` is recomputed on every hit from the stored token counts and the current rate sheet, so editing `pricing:` re-costs a warm suite without re-running it. Keying pricing would discard every entry the day a vendor changed a price. (`exec` children report their own cost, which domarinn cannot re-derive, so theirs is replayed as stored.)
- **`--repeat` still samples the judge.** The trial index is in the key, so N repeats produce N independent verdicts on the first run and replay those N afterwards. Without that, two trials whose provider responses were byte-identical — common at temperature 0 — would collapse into one verdict and erase exactly the variance `--repeat` exists to measure.
- **A `similar` verdict is keyed on the embedding model.** A cosine value is a property of the model that produced the vectors, so switching embedders re-embeds rather than replaying the previous model's answers.
- **The model a provider *reports* having used is not in the key.** It cannot be: the key is derived from a request, and a reported model only exists on a response. The *requested* model is already covered. Hashing the reported one would silently discard every cached entry the day a vendor rolls a snapshot; `CaseResult.model` makes that drift visible and diffable instead, which is the useful lever.
- **`latency` assertions bypass the cache**, since a cached latency is meaningless. `cost` and `tokens` come from the stored response, so those are honored on a hit.

Each key ingredient is scoped differently, and that scope is what decides how far a change reaches:

| Ingredient | Scope | A change busts |
|---|---|---|
| provider fingerprint (incl. its `cache_salt`) | the whole provider | every case |
| rendered prompt | one prompt × case cell | the cells using that prompt |
| rendered vars | one case | that case |
| declared `tools` | the whole provider | every case |
| per-case `cache_salt` | one case | that case |
| repeat index | one trial | that trial |

Only the fingerprint is shared by every case — so putting a value there that changes often (a digest of *all* your prompts, say) throws away the whole cache on every edit. That is what per-case salts exist to avoid.

### `exec` providers and the provider salt

An `exec` provider is cached like every other kind, and its fingerprint is `command` plus `env` plus any `cache_salt` — the same shape as `model` plus `base_url` for an `anthropic` provider. Both name the thing that will answer.

The program's own bytes are deliberately **not** in it. They used to be, and the cost was that no two machines could share an `exec` entry: a fresh clone, a different working directory, or a CI runner that compiled its own provider produced a different key for an identical question. Nothing was wrong with those cached answers; they were simply unreachable, and got paid for again.

The trade is that domarinn cannot tell one build of `./sut` from the next. **`cache_salt` is how you tell it:**

```yaml
providers:
  - id: renderer
    type: exec
    command: ["./target/release/appd", "render"]
    cache_salt: "abc1234"     # bump when this is a different build
```

Keep it a **version pin**. It answers one question: *is this the same build?* In CI that is usually the commit SHA, which is more honest than hashing the binary — Rust builds are not byte-reproducible, so two runners compiling identical source disagree about the artifact while agreeing about the version. `"$digest: src/**/*.rs"` works too, and pins to the source rather than to what came out of the compiler.

**A forgotten pin is reported, not silent.** domarinn records a digest of the program *on the entry* — never in the key — and compares it on a hit. When they disagree you get:

```
warn  provider `renderer`: replaying cached answers produced by a different
      build of this provider's program. The cache key names the command, not
      its bytes, so a rebuild does not invalidate anything on its own — set
      `cache_salt` when a rebuild should re-run the suite.
```

Nothing is thrown away, because deciding a rebuild matters is the suite's call rather than the filesystem's. A command where no argument names a readable file (`docker run …`, a shell builtin) has no digest to compare, so there is no warning available there — `cache_salt` is the only signal, exactly as it was before.

#### The child's environment is only keyed when you declare it

The fingerprint covers the provider's **declared** `env` (as a digest, never the values). But the child also inherits domarinn's own environment, and *that* half is invisible to the cache key. A variable the program reads for itself, without the suite declaring it, changes behavior while every cache entry stays valid — so two runs that differ only in an exported variable will replay each other's answers.

Put anything that steers behavior where the fingerprint can see it, either as an argument or in `env`. [`${env:VAR}` interpolation](#which-env-syntax) is how you source it from the ambient environment and still have it keyed:

```yaml
providers:
  - id: agent
    type: exec
    command: ["my-agent", "--model", "${env:AGENT_MODEL:-sonnet}"]
```

`AGENT_MODEL=opus` now changes the model *and* the cache key together. An unset variable with no `:-default` is a hard load error, not a silent fallback.

### Which `env` syntax {#which-env-syntax}

A suite can read the environment two ways, and they behave differently for caching. This is the single most common way to get a silent stale replay, so it is worth knowing which one you are using:

| Syntax | Resolved | In the cache key? | Use it for |
|---|---|---|---|
| `${env:VAR}` | at load time, before the provider is built | **yes** — the substituted value is in the fingerprint | anything that changes the answer: a model, an endpoint, a mode |
| `{{ env.VAR }}` | at call time, per request | **no** — only the unrendered template is | credentials |

The split is deliberate. A credential must *not* separate two teammates' entries, or a shared cache silently becomes a private one per API key. A model selector must separate them, or two models share one set of answers. domarinn cannot tell which is which, so it does not guess: `${env:VAR}` is keyed, `{{ env.VAR }}` is not, and an `http` provider whose url, headers or body reference `{{ env.X }}` warns at startup pointing at the keyed form.

### Per-case salts

A test case may carry its own `cache_salt`. It joins that case's key and nothing else, so changing it re-runs exactly one case:

```yaml
tests:
  - id: refuses-out-of-scope
    vars: {prompt_id: pentest-session, user_message: "scan 10.0.0.1"}
    cache_salt: "a91f3c…"      # digest of the prompt THIS case exercises
  - id: severity-reasoning
    vars: {prompt_id: triage, user_message: "rate this finding"}
    cache_salt: "77b0de…"      # a different prompt, a different digest
```

Reach for it when the system under test loads content domarinn cannot see — an `exec` provider that resolves its own prompt templates from disk by id, for example. domarinn has no way to notice those files changed, so without a salt it would serve a stale response. Compute the digest over just the content that case actually depends on.

#### Letting domarinn compute the digest

Writing those digests by hand does not scale, and computing them outside the suite means a build step — in practice, a whole test generator whose only job is injecting one field per case. `$digest:` does it for you:

```yaml
tests:
  - id: refuses-out-of-scope
    vars: {prompt_id: pentest-session, user_message: "scan 10.0.0.1"}
    cache_salt: "$digest: prompts/{{ prompt_id }}.md"
```

The glob is rendered against the case's own vars, so each case digests exactly the file it exercises rather than a constant that busts the whole suite on every edit. Matched files are hashed in sorted order **with their relative paths**, so moving content between two matched files counts as a change. A glob that matches nothing is an error, not an empty digest — an empty digest would be one constant salt shared by every such case, which is no separation at all wearing a hash. Paths are sandboxed to the suite directory, like every other file reference.

You do **not** need this for ordinary suites: an edit to a `prompts:` template or to a case's `vars` already changes the rendered request, which is already in the key.

Two rules worth stating plainly:

- **A `cache_salt` is a version pin, not an entry ticket.** Every provider is cached by default, including `exec`. The key names what will answer — `command` and `env` for an `exec` provider — and hashes what is asked. It says nothing about the program's bytes, which is what makes an entry reusable on another machine, and is also why domarinn cannot notice a rebuild by itself. Setting `cache_salt` is how a suite says "different version, throw the old answers away"; leaving it unset means a rebuild is reported on the hit instead.
- **The salt is never sent to the provider** and is never templated — it is used verbatim. Putting the digest in `vars` instead would work, but `vars` are forwarded to the system under test and enter the template namespace.

`defaults.cache_salt` fills in for cases that set none, including generator-produced ones. Treat it as a fallback only: a single constant there is shared by every case, which reproduces exactly the all-or-nothing busting that per-case salts exist to avoid. When a generator knows which content each case depends on, have it emit a `cache_salt` per case — including a `$digest:` one, which is resolved for generated cases exactly as it is for inline ones.

In CSV/TSV test files, `cache_salt` is a **reserved column** (like `id` and `tags`), so it keys the cache instead of becoming a var.

## Cache modes

| Mode | How | Behavior |
|------|-----|----------|
| read-write | default | Read on hit, write on miss. |
| disabled | `run --no-cache` | Never read or write. |
| read-only strict | `run --cache-only` | Read only; a miss is an infrastructure error (exit `3`). Use for fully offline/reproducible CI. |

`--cache-only` really is offline: the pre-run credential check is skipped, so the run needs no API keys in the environment for providers it will only replay. A grading that is *not* verdict-cached in that run (`cache.grader: false`, `--no-grader-cache`) is a miss rather than a live judge call, for the same reason — a `--cache-only` run that quietly reached the network would be lying about being offline.

## Backends

The backend **type** is chosen in the suite `cache:` block; URLs and credentials come from flags/environment, so no secrets live in the checked-in YAML.

```yaml
cache:
  backend: disk | http | s3 | layered
  s3:                         # only for s3 / layered-with-s3
    bucket: domarinn-cache
    endpoint: https://s3.example      # optional, for MinIO/Garage/etc.
    region: us-east-1
    prefix: team-a
    force_path_style: true            # MinIO/Garage typically need this
```

| Backend | What it is |
|---------|-----------|
| `disk` (default) | A local content-addressed store at `.domarinn/cache` beside the suite, one file per entry (written to a temp file then atomically renamed, so it is safe under concurrent runs and `rsync`/`s3 sync`). |
| `http` | The domarinn server as a shared read-through cache, **behind the local disk tier** — identical to `layered` without S3. Needs a server URL (`--server-url` / `DOMARINN_SERVER_URL`) and, if the server requires auth, `DOMARINN_TOKEN`. Zero extra setup — the same URL you share runs to. |
| `s3` | Any S3-compatible bucket via the standard AWS credential chain, **behind the local disk tier** — identical to `layered` with `cache.s3`. Writes are additive and first-write-wins (an existing object is left alone, never overwritten or deleted); retention is the bucket's lifecycle rules. Works with AWS, MinIO, Garage, SeaweedFS. |
| `layered` | A read-through pairing of the fast local disk cache and a shared remote (S3 if `cache.s3` is set, else the HTTP server). Reads try local, then remote (populating local on a hit); writes go to local synchronously and to the remote best-effort. |

Every remote backend keeps the local disk tier in front of it, so a warm rerun is served locally and never reaches the network. To exercise or measure the remote path, point `--cache-dir` at an empty directory or run `domarinn cache clear` first.

If a remote backend is selected but its server URL or credentials are missing (for example a fresh clone with no environment), domarinn **falls back to local disk with a warning** rather than failing the run.

## Sharing cache between teammates

Point everyone at the same shared backend and the whole team reuses each other's provider responses:

- **Via the server** — set `cache.backend: layered` in the suite and `DOMARINN_SERVER_URL` (+ `DOMARINN_TOKEN`) in each environment. The first person to run a case pays for it; everyone else gets a hit.
- **Via S3** — set `cache.backend: s3` (or `layered`) with a `cache.s3` block and provide bucket credentials through the AWS chain.

For sharing to hold, keep the provider *configuration* identical across environments: the key includes the provider fingerprint, so a different model, `base_url`, params, headers, or `cache_salt` is simply a different entry that nobody else hits.

Nothing about the *environment* has to match. Different checkout paths, different file timestamps, a binary rebuilt from the same commit, a different working directory, unrelated exported variables — none of these move a key, by construction. That is what makes a shared backend worth having, and `crates/domarinn-core/tests/cache_portability.rs` pins each one so it stays true. Grader verdicts are cached too, in their own key space keyed on the grader's fingerprint and the graded payload, so a warm run re-pays neither the provider nor the judge. `--no-grader-cache` re-grades while still replaying provider responses, which is how you measure judge variance deliberately. See [grading.md](./grading.md).

## Managing the local cache

```sh
domarinn cache stats ./evals         # entry count and total size for a suite
domarinn cache path ./evals          # print the cache directory
domarinn cache gc --older-than 30d   # remove entries older than 30 days
domarinn cache clear                 # remove everything
```

Durations accept `d`, `h`, `m`, `s` (e.g. `12h`, `90s`).

### Where the cache lives

`.domarinn/cache` **beside the suite**, matching how every other path in a run resolves — `file://`, `$digest:`, and the directory an `exec` child is spawned in. Two overrides, in order:

| | |
|---|---|
| `--cache-dir DIR` | wins over everything; point it at a directory CI restores to reuse a warm cache across jobs |
| `DOMARINN_CACHE_DIR` | same thing from the environment |
| *(default)* | `<suite dir>/.domarinn/cache` |

The `cache` subcommands take the same suite path and the same `--cache-dir`, so they inspect the directory a run would actually use.

> **Upgrading:** this used to resolve against the *process* working directory, so running `domarinn run evals/suite.yaml` from a repo root and `domarinn run suite.yaml` from `evals/` used two different caches for identical work. If you ran from a parent directory, your entries are in that parent's `.domarinn/cache`; domarinn reads it automatically as a read-only tier and copies entries forward as they are used, or you can point `--cache-dir` straight at it.

### Upgrading across a key change

Changing the *shape* of a fingerprint changes every key derived from it, which would strand a whole store of good answers. domarinn migrates them instead: on a miss it re-derives the key from each shape it used to publish, and adopts the first hit — serving it and re-filing it under the current key, so the next run finds it directly.

This is self-limiting. A store with nothing to migrate stops being probed after a handful of cases, so the cost is a few extra lookups once rather than a permanent tax. `--no-cache-migration` skips it entirely, which is worth doing only against a high-latency remote you know has nothing to adopt.

## Server cache endpoints

When the server acts as the shared cache, it exposes (under `/api/v1`):

| Method | Path | Scope | Notes |
|--------|------|-------|-------|
| `GET` / `HEAD` | `/cache/{key}` | read | Fetch / existence check. `{key}` is `sha256:<hex>`. |
| `PUT` | `/cache/{key}` | write | Immutable: `201` on create, `200` if it already exists. Oversized bodies get `413`. |
| `GET` | `/cache/stats` | read | Entry count, bytes, hit/miss counters. |
| `POST` | `/cache/prune` | admin | Prune by age or target size. |

The server enforces size and age limits and prunes least-recently-used entries in the background. See [server.md](./server.md) for auth and env vars.
