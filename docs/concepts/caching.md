# Caching

domarinn caches every call it makes, so re-running a suite is fast, cheap, and deterministic — and so a team can share that work.

## The rule {#the-rule}

**One rule.** Every outgoing request — a provider call, the LLM grader, an embeddings call, an `exec` grader — is cached under one key: the SHA-256 of the redacted request itself, plus the trial index, plus any `cache_salt` in scope. Nothing about your machine, your binary, or your credentials is in it.

**Hash what is sent.** There is no second rule for graders, no separate key space for verdicts, and no ingredient that describes *where* a request was made from. Two calls that would put the same bytes on the wire are the same call, and they share one entry.

That is what makes a cache shareable. A key that varies by machine cannot be reused by anyone else, which quietly turns the S3 and results-server backends into an expensive local disk. Because the key is a pure function of the request, the same question keys the same way in every checkout, on every machine, from any working directory.

### What is in the key

| Ingredient | HTTP-shaped requests | `exec`-shaped requests | A change busts |
|---|---|---|---|
| **the request** | the resolved URL and the body — call-time `{{ env.* }}` rendered to a `${env:NAME}` placeholder, headers reduced to a digest, `anthropic`'s API-version header carried along, an `http` provider's `output_expr` too | the command and its args, the stdin document minus its `test` block, and a digest of the declared `env` | every call that request appears in |
| **the trial index** | the `--repeat` number | same | one trial |
| **a salt in scope** | the provider's or the case's `cache_salt`, when set | the same two on a provider call; on an `exec` **assertion's** gradings, that assertion's own `cache_salt` and nothing else | exactly what that salt scopes |

A salt joins the hash **only when it is set**, which is what keeps every key written before salts existed valid.

### Every knob, once

| Knob | Where | Scope |
|---|---|---|
| `providers[].cache_salt` | suite | every request that provider answers |
| `tests[].cache_salt` (a reserved CSV/TSV column) | suite / dataset | one case's provider requests |
| `defaults.cache_salt` | suite | cases that set none, generator-produced included |
| an `exec` assertion's `cache_salt` | suite | that assertion's grading requests |
| `cache.backend: disk \| layered` | suite | which store (`http` and `s3` are deprecated aliases) |
| `cache.s3.*` | suite | the S3 settings `layered` uses |
| `--cache-dir` / `DOMARINN_CACHE_DIR` | run | where the local tier lives |
| `--no-cache` / `--cache-only` / `--no-cache-migration` | run | mode |
| `--no-grader-cache` (`cache.grader` is deprecated) | run | grader-originated requests |

That is the complete list. Every other page links here rather than re-deriving it.

## What is and is not cached

- **One entry per key, immutable.** The first write wins, on every backend. Two writers who agree on a key agree on the *answer* — though not on the bytes, since an entry also records when the call happened, how many attempts it took, which version wrote it, and the request it answers.
- **Errors are never cached.** Only successful responses are stored.
- **Grader calls are requests like any other.** The grader's HTTP call, an embedding, and an `exec` assertion's protocol round-trip all go through the same key function as a provider call. An LLM-graded suite used to re-pay its grader on every run even when every provider response was a hit, which is the dominant recurring cost of running one. `--no-grader-cache` turns just those off.
- **What is stored for a grading is the response, not the verdict.** The verdict is re-derived on read, by the same parser a live call uses — so a replay can never produce a shape today's code would reject, and a parser fix applies retroactively to everything already stored. A stored payload that no longer parses is a **miss**, never an error: it is re-asked, and warned about, because an immutable entry could otherwise fail the same assertion forever.
- **A `threshold` is not in the key.** The threshold is applied on read, so editing a `threshold:` re-scores every case instantly instead of re-paying the grader for an answer it already gave.
- **Neither is pricing.** `cost_usd` is recomputed on every hit from the stored token counts and the current rate sheet, so editing `pricing:` re-costs a warm suite without re-running it. Keying pricing would discard every entry the day a vendor changed a price. (`exec` children report their own cost, which domarinn cannot re-derive, so theirs is replayed as stored.)
- **`--repeat` still samples the grader.** The trial index is in the key, so N repeats produce N independent verdicts on the first run and replay those N afterwards. Without that, two trials whose provider responses were byte-identical — common at temperature 0 — would collapse into one verdict and erase exactly the variance `--repeat` exists to measure.
- **`similar` caches two embeddings, not a similarity.** Each side is keyed on its own embedding request, which names the model — so switching embedders re-embeds, and an output measured against a second reference re-embeds only the reference.
- **The model a provider *reports* having used is not in the key.** It cannot be: the key is derived from a request, and a reported model only exists on a response. The *requested* model is already in the body. Hashing the reported one would silently discard every cached entry the day a vendor rolls a snapshot; `CaseResult.model` makes that drift visible and diffable instead, which is the useful lever.
- **Test ids and tags are not in the key.** Identity is not what makes two calls interchangeable — the request is. Two cases with identical vars and no prompt therefore share an entry by design; a per-case [`cache_salt`](#per-case-salts) is the supported way to separate them.
- **A prompt template's source text is not in the key.** The *rendered* prompt is already inside the request, so hashing the template as well would only bust on edits that render identically — a change with no effect on the provider.
- **An `http` provider's `output_expr` *is* in the key**, even though it never goes on the wire. The entry stores the already-projected output, so the expression decides what the stored answer means: two providers reading different fields out of one endpoint's response are not asking the same question. Editing it busts that provider's entries like any other change to the request — no `--no-cache` needed.
- **`latency` assertions bypass the cache**, since a cached latency is meaningless. `cost` and `tokens` come from the stored response, so those are honored on a hit. Under `--cache-only` that bypass would be a live call in the one mode documented as offline, so the case is [refused instead](#cache-modes).

## Salts

### Per-case salts {#per-case-salts}

A test may carry its own `cache_salt`. It joins the key of every case that test produces and nothing else, so changing it re-runs only those:

```yaml
tests:
  - id: refuses-out-of-scope
    vars: {prompt_id: pentest-session, user_message: "scan 10.0.0.1"}
    cache_salt: "a91f3c…"      # digest of the prompt THIS test exercises
  - id: severity-reasoning
    vars: {prompt_id: triage, user_message: "rate this finding"}
    cache_salt: "77b0de…"      # a different prompt, a different digest
```

Reach for it when the system under test loads content domarinn cannot see — an `exec` provider that resolves its own prompt templates from disk by id, for example. domarinn has no way to notice those files changed, so without a salt it would serve a stale response. Compute the digest over just the content that case actually depends on.

#### Letting domarinn compute the digest

Writing those digests by hand does not scale, and computing them outside the suite means a build step — in practice, a whole test generator whose only job is injecting one field per case. `$digest:` does it for you — from a shipped example, one case per prompt plus a third that digests a whole glob of them:

```yaml
--8<-- "examples/22-cache-salts/domarinn.yaml:salts"
```

The glob is rendered against the case's own vars, so each case digests exactly the file it exercises rather than a constant that busts the whole suite on every edit. Matched files are hashed in sorted order **with their relative paths**, so moving content between two matched files counts as a change. A glob that matches nothing is an error, not an empty digest — an empty digest would be one constant salt shared by every such case, which is no separation at all wearing a hash. Paths are sandboxed to the suite directory, like every other file reference.

You do **not** need this for ordinary suites: an edit to a `prompts:` template or to a case's `vars` already changes the request, which is already in the key.

Two rules worth stating plainly:

- **A `cache_salt` is a version pin, not an entry ticket.** Every provider is cached by default, including `exec`. Setting a salt is how a suite says "different version, throw the old answers away"; leaving it unset means a rebuild is reported on the hit instead.
- **The salt is never sent to the provider** and is never templated — it is used verbatim. Putting the digest in `vars` instead would work, but `vars` are forwarded to the system under test and enter the template namespace.

`defaults.cache_salt` fills in for cases that set none, including generator-produced ones. Treat it as a fallback only: a single constant there is shared by every case, which reproduces exactly the all-or-nothing busting that per-case salts exist to avoid. When a generator knows which content each case depends on, have it emit a `cache_salt` per case — including a `$digest:` one, which is resolved for generated cases exactly as it is for inline ones.

In CSV/TSV test files, `cache_salt` is a **reserved column** (like `id` and `tags`), so it keys the cache instead of becoming a var.

### `exec` providers and the provider salt

An `exec` provider is cached like every other kind: its request is the command, the args, the protocol document on the child's stdin, and a digest of the `env` the suite declares.

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
      `cache_salt` (a git SHA, or `$digest:` over the sources) when a rebuild
      should re-run the suite.
```

Nothing is thrown away, because deciding a rebuild matters is the suite's call rather than the filesystem's. A command where no argument names a readable file (`docker run …`, a shell builtin) has no digest to compare, so there is no warning available there — `cache_salt` is the only signal.

#### The child's environment is only keyed when you declare it {#the-childs-environment-is-only-keyed-when-you-declare-it}

The request carries the provider's **declared** `env` as a digest, never the values. But the child also inherits domarinn's own environment, and *that* half is invisible to the cache key. A variable the program reads for itself, without the suite declaring it, changes behavior while every cache entry stays valid — so two runs that differ only in an exported variable will replay each other's answers.

Put anything that steers behavior where the key can see it, either as an argument or in `env`. [`${env:VAR}` interpolation](#which-env-syntax) is how you source it from the ambient environment and still have it keyed:

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
| `${env:VAR}` | at load time, before the provider is built | **yes** — the substituted value is part of the request | anything that changes the answer: a model, an endpoint, a mode |
| `{{ env.VAR }}` | at call time, per request | **no** — the request is keyed with the value replaced by a literal `${env:NAME}` placeholder | credentials |

The split is deliberate. A credential must *not* separate two teammates' entries, or a shared cache silently becomes a private one per API key. A model selector must separate them, or two models share one set of answers. domarinn cannot tell which is which, so it does not guess: `${env:VAR}` is keyed, `{{ env.VAR }}` is not, and an `http` provider whose url, headers or body reference `{{ env.X }}` warns at startup pointing at the keyed form.

/// danger | The placeholder covers one hop, and one only

`{{ env.X }}` is withheld where a **provider's** url, headers or body render it. A *case var* defined as `{{ env.SECRET }}` is resolved long before a provider renders anything, so its value reaches the request in the clear — it is keyed, it is stored on the entry, and it is published in `CaseResult.vars`. That is by design: vars are case data. A credential therefore belongs in a provider's own templates, never routed through a case var.

///

## Cache modes

| Mode | How | Behavior |
|------|-----|----------|
| read-write | default | Read on hit, write on miss. |
| disabled | `run --no-cache` | Never read or write. |
| read-only strict | `run --cache-only` | Read only; a miss is an infrastructure error (exit `3`), and so is a case that could only be answered live. Use for fully offline/reproducible CI. |
| graders live | `run --no-grader-cache` | Grader-originated requests skip the cache in both directions; provider responses are still replayed. |

`--cache-only` really is offline: the pre-run credential check is skipped, so the run needs no API keys in the environment for providers it will only replay. Two consequences follow from that promise:

- **`--cache-only` with grader caching off fails every graded assertion.** There is no cache to consult and no live call permitted, so the assertion errors rather than quietly reaching the grader.
- **A case with a `latency` assertion is refused outright**, per case, with `there is nothing honest to replay`. A `latency` assert always measures a live call, so honoring both the bypass and the offline promise is impossible. The rest of the suite still replays.

`cache.grader: false` in a suite is the deprecated spelling of `--no-grader-cache` and is accepted with a warning. Prefer the flag: whether to re-grade is a property of one run, not of the suite.

## Backends

The backend **type** is chosen in the suite `cache:` block; URLs and credentials come from flags/environment, so no secrets live in the checked-in YAML.

```yaml
cache:
  backend: disk | layered
  s3:                         # only for layered-with-S3
    bucket: domarinn-cache
    endpoint: https://s3.example      # optional, for MinIO/Garage/etc.
    region: us-east-1
    prefix: team-a
    force_path_style: true            # MinIO/Garage typically need this
```

| Backend | What it is |
|---------|-----------|
| `disk` (default) | A local content-addressed store at `.domarinn/cache` beside the suite, one file per entry (written to a temp file then atomically renamed, so it is safe under concurrent runs and `rsync`/`s3 sync`). |
| `layered` | A read-through pairing of the fast local disk cache and a shared remote — S3 when `cache.s3` is set, else the domarinn server (`--server-url` / `DOMARINN_SERVER_URL`, plus `DOMARINN_TOKEN` if it requires auth). Reads try local, then remote (populating local on a hit); writes go to local synchronously and to the remote best-effort, first-write-wins. |

`http` and `s3` are **deprecated aliases** for `layered`, and both warn at startup. They name one tier outright rather than letting `cache.s3` choose, which is the same thing as `layered` only when the two agree: `backend: s3` with no `cache.s3` block degrades to local disk *alone* where `layered` would have used the server, and `backend: http` ignores a `cache.s3` block that `layered` would have used. Write `layered` and let the config decide which remote you meant.

Every remote backend keeps the local disk tier in front of it, so a warm rerun is served locally and never reaches the network. To exercise or measure the remote path, point `--cache-dir` at an empty directory or run `domarinn cache clear` first.

If a remote backend is selected but its server URL, bucket config or credentials are missing (for example a fresh clone with no environment), domarinn **falls back to local disk with a warning** rather than failing the run. A misconfigured CI job therefore looks like it is working while paying full price — check for the warning.

## Sharing a cache across a team

Pointing a team and CI at the same backend, what has to match for a hit, and the salt discipline that keeps it useful are covered end to end in [Share a cache and baselines](../guides/share-cache-and-baselines.md).

## Where the cache lives

`.domarinn/cache` **beside the suite**, matching how every other path in a run resolves — `file://`, `$digest:`, and the directory an `exec` child is spawned in. Two overrides, in order:

| | |
|---|---|
| `--cache-dir DIR` | wins over everything; point it at a directory CI restores to reuse a warm cache across jobs |
| `DOMARINN_CACHE_DIR` | same thing from the environment |
| *(default)* | `<suite dir>/.domarinn/cache` |

The `cache` subcommands take the same suite path and the same `--cache-dir`, so they inspect the directory a run would actually use.

> **Upgrading:** this used to resolve against the *process* working directory, so running `domarinn run evals/suite.yaml` from a repo root and `domarinn run suite.yaml` from `evals/` used two different caches for identical work. A cwd-relative `.domarinn/cache` that still exists is layered underneath as a **read-only legacy tier**: entries found there are copied forward as they are used and never written back. An explicit `--cache-dir` is taken at face value and layers nothing.

## Managing the local cache

```sh
domarinn cache stats ./evals         # entry count and total size for a suite
domarinn cache path ./evals          # print the cache directory
domarinn cache gc --older-than 30d   # remove entries older than 30 days
domarinn cache clear                 # remove everything
```

Durations accept `d`, `h`, `m`, `s` (e.g. `12h`, `90s`).

- **`gc` requires `--older-than`.** A bare `domarinn cache gc` is a usage error, not a full purge: the obvious reading of `gc` is "tidy up a bit", and the command that removes everything should be the one that says so. Use `cache clear`.
- **These commands address the local tier only.** They do not reach an S3 bucket or the server — remote retention is the bucket's lifecycle rules and the server's [prune endpoint and hourly retention task](../reference/rest-api.md#cache-shared-provider-cache).
- **The legacy tier is reported by `stats` and `path` always, but only purged when it is yours.** `clear` and `gc` touch it only when the suite sits at or under the process working directory, because a cwd-relative `.domarinn/cache` belongs to whatever project you happen to be standing in — `cd ~/projB && domarinn cache clear ~/projA/evals` must not take projB's cache with it. `stats` says which of the two it is.

## Upgrading to 0.5

**Every key changed.** Through 0.4.x a provider entry was keyed on a provider *fingerprint* hashed alongside the pieces of the request, and graders had a second key space of their own — a grading fingerprint hashed alongside the graded document. 0.5.0 replaced both with the one rule above. A store full of perfectly good answers became unreachable: not wrong, just invisible.

So domarinn migrates instead of re-paying. Things worth knowing before the first warm run:

- **Adoption happens on a miss.** domarinn re-derives the key from each shape it used to publish, adopts the first hit, serves it, and re-files it under the current key — with the request it answers attached this time — so the next run finds it directly. It is self-limiting: a store with nothing to adopt stops being probed after a handful of cases, so the cost is a few extra lookups once rather than a permanent tax. `--no-cache-migration` skips it entirely, which is worth doing only against a high-latency remote you know has nothing to adopt. The machinery ships in 0.5.0, stays through 0.6.x, and is **deleted in 0.7.0**.
- **`similar` verdicts are not adopted.** A 0.4.x entry holds one cosine value, which decomposes into neither of the two embedding vectors 0.5.0 caches — there is nothing to adopt it *into*. Re-embedding two short strings costs a fraction of a cent once, and the assertion is warm from then on. The exception is offline: under `--cache-only` a store warmed by 0.4.x **hard-errors on every `similar` assertion** until the suite has been run once in read-write mode to lay the vectors down.
- **Entries now record the request they answer**, resolved URL or command included, so an entry is legible on its own. A request too large for the entry cap keeps a slim envelope — the address (`transport`, `method`, `url`, `command`, `args`) and nothing else.
- **Entries got bigger.** A grader entry now carries the grading model's whole response rather than a three-field verdict, and an embedding is tens of kilobytes of floats. The boundary is the shared store's: the server rejects anything over `DOMARINN_CACHE_MAX_ENTRY_BYTES` (4 MiB by default) with a `413`, which the client logs and continues past — so an oversized payload is re-paid every run rather than failing one.
- **Suites with a `cache:` block see one `config_digest` change.** An unset `cache.grader` no longer serializes, where the old boolean always wrote `true`. `config_digest` is a hash of the serialized suite, so exactly one `--against` comparison across the upgrade reports config drift. No cache entry is affected — the digest is not a key ingredient.
- **A future `exec` protocol bump is a deliberate cache flag day.** The `domarinn` envelope is inside the stdin document the key hashes, because it is genuinely sent and a child may answer a v2 request differently. Bumping `PROTOCOL_VERSION` therefore re-keys every `exec` entry in every store at once. A test asserts the current version so that decision is made on purpose, with a legacy generation frozen first — see [protocol.md](../reference/protocol.md#versioning).

## The server as the shared tier

When the domarinn server backs a `layered` cache, it serves the entries over `/api/v1/cache/*` and prunes them on a schedule. The wire contract, the size and age limits, and the auth scopes are in [rest-api.md](../reference/rest-api.md#cache-shared-provider-cache).
