# Caching

domarinn caches provider responses so re-running a suite is fast, cheap, and
deterministic — and so a team can share that work.

## How the cache works

The cache is **content-addressed**. The key is a SHA-256 of the provider's
identity (its fingerprint — type, model/command/url, params) plus the rendered
request, the case's [`cache_salt`](#per-case-salts) when it sets one, and the
repeat index. Same input, same key; a change to the model, params, prompt, or
vars produces a new key.

- **One entry per key, immutable.** The first write wins. Because entries are
  content-addressed, two machines that compute the same request write identical
  bytes, so sharing is safe with no clobbering.
- **Errors are never cached.** Only successful responses are stored.
- **Grader verdicts are cached too**, on by default like everything else. An
  LLM-graded suite used to re-pay its judge on every run even when every
  provider response was a cache hit, which is the dominant recurring cost of
  running one. Disable with `cache.grader: false` or `--no-grader-cache`.
- **A `threshold` is not in the verdict key.** The cached value is the raw
  verdict, and the threshold is applied on read — so editing a `threshold:`
  re-scores every case instantly instead of re-paying the judge for an answer
  it already gave. This is structural: the cached type has no threshold in it.
- **`--repeat` still samples the judge.** The trial index is in the key, so N
  repeats produce N independent verdicts on the first run and replay those N
  afterwards. Without that, two trials whose provider responses were
  byte-identical — common at temperature 0 — would collapse into one verdict
  and erase exactly the variance `--repeat` exists to measure.
- **A `similar` verdict is keyed on the embedding model.** A cosine value is a
  property of the model that produced the vectors, so switching embedders
  re-embeds rather than replaying the previous model's answers.
- **The model a provider *reports* having used is not in the key.** It cannot
  be: the key is derived from a request, and a reported model only exists on a
  response. The *requested* model is already covered (it is in the
  `anthropic`/`openai` fingerprints, and inside `command` for `exec`). Hashing
  the reported one would silently discard every cached entry the day a vendor
  rolls a snapshot; `CaseResult.model` makes that drift visible and diffable
  instead, which is the useful lever.
- **`latency` assertions bypass the cache**, since a cached latency is
  meaningless. `cost` and `tokens` come from the stored response, so those are
  honored on a hit.

Each key ingredient is scoped differently, and that scope is what decides how far
a change reaches:

| Ingredient | Scope | A change busts |
|---|---|---|
| provider fingerprint (incl. its `cache_salt`) | the whole provider | every case |
| rendered prompt | one prompt × case cell | the cells using that prompt |
| rendered vars | one case | that case |
| per-case `cache_salt` | one case | that case |
| repeat index | one trial | that trial |

Only the fingerprint is shared by every case — so putting a value there that
changes often (a digest of *all* your prompts, say) throws away the whole cache
on every edit. That is what per-case salts exist to avoid.

### `exec` providers and the provider salt

An `exec` provider is cached like every other kind. Its fingerprint includes the
identity of the *program* — the path, size and modification time of every
argument that names a readable file — not just the command line, so rebuilding
the binary busts its entries automatically.

That covers a compiled binary (`./target/release/appd`) and an interpreter plus
a script (`python3 grade.py`). It does **not** cover a program resolved from
`PATH` with no path separator, or one whose behavior depends on something not on
disk. Set `cache_salt` for those — it composes with the automatic identity
rather than replacing it:

```yaml
providers:
  - id: renderer
    type: exec
    command: ["./target/release/appd", "render"]
    cache_salt: "abc1234"     # bump on rebuild; without it, exec is not cached
```

Keep this a **version pin**. It answers one question: *is this the same build?*

### Per-case salts

A test case may carry its own `cache_salt`. It joins that case's key and nothing
else, so changing it re-runs exactly one case:

```yaml
tests:
  - id: refuses-out-of-scope
    vars: {prompt_id: pentest-session, user_message: "scan 10.0.0.1"}
    cache_salt: "a91f3c…"      # digest of the prompt THIS case exercises
  - id: severity-reasoning
    vars: {prompt_id: triage, user_message: "rate this finding"}
    cache_salt: "77b0de…"      # a different prompt, a different digest
```

Reach for it when the system under test loads content domarinn cannot see — an
`exec` provider that resolves its own prompt templates from disk by id, for
example. domarinn has no way to notice those files changed, so without a salt it
would serve a stale response. Compute the digest over just the content that case
actually depends on.

#### Letting domarinn compute the digest

Writing those digests by hand does not scale, and computing them outside the
suite means a build step — in practice, a whole test generator whose only job is
injecting one field per case. `$digest:` does it for you:

```yaml
tests:
  - id: refuses-out-of-scope
    vars: {prompt_id: pentest-session, user_message: "scan 10.0.0.1"}
    cache_salt: "$digest: prompts/{{ prompt_id }}.md"
```

The glob is rendered against the case's own vars, so each case digests exactly
the file it exercises rather than a constant that busts the whole suite on every
edit. Matched files are hashed in sorted order **with their relative paths**, so
moving content between two matched files counts as a change. A glob that matches
nothing is an error, not an empty digest — an empty digest would be one constant
salt shared by every such case, which is no separation at all wearing a hash.
Paths are sandboxed to the suite directory, like every other file reference.

You do **not** need this for ordinary suites: an edit to a `prompts:` template or
to a case's `vars` already changes the rendered request, which is already in the
key.

Two rules worth stating plainly:

- **An `exec` provider is cached by default, and a `cache_salt` is the escape
  hatch rather than the entry ticket.** What makes that safe is that the cache
  key includes the *program*, not just the argv that names it: the path, size and
  modification time of every argument that resolves to a file, with the program
  itself resolved through `PATH` and relative paths resolved against the suite
  directory (the cwd the child is spawned in). A rebuild therefore busts the
  entry on its own. When nothing in the command resolves to a file — `docker run
  …`, say — there is no identity beyond argv, and domarinn declines to cache
  rather than replay stale output after a rebuild; set `cache_salt` to say "I
  know what moves this" and caching resumes. The provider's `env` is in the key
  too (as a digest, never the values), so two providers wrapping one script with
  different endpoints do not collide.
- **The salt is never sent to the provider** and is never templated — it is used
  verbatim. Putting the digest in `vars` instead would work, but `vars` are
  forwarded to the system under test and enter the template namespace.

`defaults.cache_salt` fills in for cases that set none, including
generator-produced ones. Treat it as a fallback only: a single constant there is
shared by every case, which reproduces exactly the all-or-nothing busting that
per-case salts exist to avoid. When a generator knows which content each case
depends on, have it emit a `cache_salt` per case — including a `$digest:` one,
which is resolved for generated cases exactly as it is for inline ones.

In CSV/TSV test files, `cache_salt` is a **reserved column** (like `id` and
`tags`), so it keys the cache instead of becoming a var.

## Cache modes

| Mode | How | Behavior |
|------|-----|----------|
| read-write | default | Read on hit, write on miss. |
| disabled | `run --no-cache` | Never read or write. |
| read-only strict | `run --cache-only` | Read only; a miss is an infrastructure error (exit `3`). Use for fully offline/reproducible CI. |

`--cache-only` really is offline: the pre-run credential check is skipped, so the
run needs no API keys in the environment for providers it will only replay. A
grading that is *not* verdict-cached in that run (`cache.grader: false`,
`--no-grader-cache`) is a miss rather than a live judge call, for the same
reason — a `--cache-only` run that quietly reached the network would be lying
about being offline.

## Backends

The backend **type** is chosen in the suite `cache:` block; URLs and credentials
come from flags/environment, so no secrets live in the checked-in YAML.

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
| `disk` (default) | A local content-addressed store at `.domarinn/cache`, one file per entry (written to a temp file then atomically renamed, so it is safe under concurrent runs and `rsync`/`s3 sync`). |
| `http` | The domarinn server as a shared read-through cache, **behind the local disk tier** — identical to `layered` without S3. Needs a server URL (`--server-url` / `DOMARINN_SERVER_URL`) and, if the server requires auth, `DOMARINN_TOKEN`. Zero extra setup — the same URL you share runs to. |
| `s3` | Any S3-compatible bucket via the standard AWS credential chain, **behind the local disk tier** — identical to `layered` with `cache.s3`. Writes are additive-only (never delete or overwrite); retention is the bucket's lifecycle rules. Works with AWS, MinIO, Garage, SeaweedFS. |
| `layered` | A read-through pairing of the fast local disk cache and a shared remote (S3 if `cache.s3` is set, else the HTTP server). Reads try local, then remote (populating local on a hit); writes go to local synchronously and to the remote best-effort. |

Every remote backend keeps the local disk tier in front of it, so a warm rerun
from the same working directory is served locally and never reaches the network.
To exercise or measure the remote path, use a fresh working directory or run
`domarinn cache clear` first.

If a remote backend is selected but its server URL or credentials are missing
(for example a fresh clone with no environment), domarinn **falls back to
local disk with a warning** rather than failing the run.

## Sharing cache between teammates

Point everyone at the same shared backend and the whole team reuses each other's
provider responses:

- **Via the server** — set `cache.backend: layered` in the suite and
  `DOMARINN_SERVER_URL` (+ `DOMARINN_TOKEN`) in each environment. The first
  person to run a case pays for it; everyone else gets a hit.
- **Via S3** — set `cache.backend: s3` (or `layered`) with a `cache.s3` block and
  provide bucket credentials through the AWS chain.

For sharing to hold, keep the provider configuration identical across
environments: the key includes the provider fingerprint, so a different model,
`base_url`, params, or `cache_salt` is simply a different entry that nobody else
hits. Note that grading is not cached, so an LLM-graded suite still pays its
grader on every run no matter how warm the response cache is. See
[grading.md](./grading.md).

## Managing the local cache

```sh
domarinn cache stats                 # entry count and total size
domarinn cache path                  # print the cache directory
domarinn cache gc --older-than 30d   # remove entries older than 30 days
domarinn cache clear                 # remove everything
```

Durations accept `d`, `h`, `m`, `s` (e.g. `12h`, `90s`).

## Server cache endpoints

When the server acts as the shared cache, it exposes (under `/api/v1`):

| Method | Path | Scope | Notes |
|--------|------|-------|-------|
| `GET` / `HEAD` | `/cache/{key}` | read | Fetch / existence check. `{key}` is `sha256:<hex>`. |
| `PUT` | `/cache/{key}` | write | Immutable: `201` on create, `200` if it already exists. Oversized bodies get `413`. |
| `GET` | `/cache/stats` | read | Entry count, bytes, hit/miss counters. |
| `POST` | `/cache/prune` | admin | Prune by age or target size. |

The server enforces size and age limits and prunes least-recently-used entries
in the background. See [server.md](./server.md) for auth and env vars.
