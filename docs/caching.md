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
- **Grader verdicts are _not_ cached.** Only provider responses are. An
  LLM-graded suite re-pays for grading on every run, even when every provider
  response is a cache hit.
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

An `exec` provider is only cached when it sets a `cache_salt`. Its fingerprint is
the command, which does **not** change when you rebuild the program behind it —
so without a salt a rebuilt binary would be served stale output. Set
`cache_salt` to something that changes with the program (a git SHA, a build
hash):

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

You do **not** need this for ordinary suites: an edit to a `prompts:` template or
to a case's `vars` already changes the rendered request, which is already in the
key.

Two rules worth stating plainly:

- **The provider salt decides _whether_ exec responses are cached; a case salt
  decides only _which key_ they are cached under.** A case salt on its own does
  not enable caching — an `exec` provider still needs its own `cache_salt`. The
  runner warns when a suite sets case salts against a provider that never caches.
- **The salt is never sent to the provider** and is never templated — it is used
  verbatim. Putting the digest in `vars` instead would work, but `vars` are
  forwarded to the system under test and enter the template namespace.

`defaults.cache_salt` fills in for cases that set none, including
generator-produced ones. Treat it as a fallback only: a single constant there is
shared by every case, which reproduces exactly the all-or-nothing busting that
per-case salts exist to avoid. When a generator knows which content each case
depends on, have it emit a `cache_salt` per case.

In CSV/TSV test files, `cache_salt` is a **reserved column** (like `id` and
`tags`), so it keys the cache instead of becoming a var.

## Cache modes

| Mode | How | Behavior |
|------|-----|----------|
| read-write | default | Read on hit, write on miss. |
| disabled | `run --no-cache` | Never read or write. |
| read-only strict | `run --cache-only` | Read only; a miss is an infrastructure error (exit `3`). Use for fully offline/reproducible CI. |

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
