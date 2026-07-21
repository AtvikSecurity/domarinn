# Caching

domarinn caches provider responses and grader verdicts so re-running a suite
is fast, cheap, and deterministic — and so a team can share that work.

## How the cache works

The cache is **content-addressed**. The key is a SHA-256 of the provider's
identity (its fingerprint — type, model/command/url, params) plus the rendered
request and the repeat index. Same input, same key; a change to the model,
params, prompt, or vars produces a new key.

- **One entry per key, immutable.** The first write wins. Because entries are
  content-addressed, two machines that compute the same request write identical
  bytes, so sharing is safe with no clobbering.
- **Errors are never cached.** Only successful responses are stored.
- **Grader verdicts are cached** like any other model call — re-grading unchanged
  output is free.
- **`latency` assertions bypass the cache**, since a cached latency is
  meaningless. `cost` and `tokens` come from the stored response, so those are
  honored on a hit.

### `exec` providers and `cache_salt`

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
| `http` | The domarinn server acts as a shared read-through cache. Needs a server URL (`--server-url` / `DOMARINN_SERVER_URL`) and, if the server requires auth, `DOMARINN_TOKEN`. Zero extra setup — the same URL you share runs to. |
| `s3` | Any S3-compatible bucket via the standard AWS credential chain. Writes are additive-only (never delete or overwrite); retention is the bucket's lifecycle rules. Works with AWS, MinIO, Garage, SeaweedFS. |
| `layered` | A read-through pairing of the fast local disk cache and a shared remote (S3 if `cache.s3` is set, else the HTTP server). Reads try local, then remote (populating local on a hit); writes go to local synchronously and to the remote best-effort. |

If a remote backend is selected but its server URL or credentials are missing
(for example a fresh clone with no environment), domarinn **falls back to
local disk with a warning** rather than failing the run.

## Sharing cache between teammates

Point everyone at the same shared backend and the whole team reuses each other's
LLM responses and grader verdicts:

- **Via the server** — set `cache.backend: layered` in the suite and
  `DOMARINN_SERVER_URL` (+ `DOMARINN_TOKEN`) in each environment. The first
  person to run a case pays for it; everyone else gets a hit.
- **Via S3** — set `cache.backend: s3` (or `layered`) with a `cache.s3` block and
  provide bucket credentials through the AWS chain.

For sharing to hold, keep grading **deterministic**: pin the grader model and use
`temperature: 0` (or no sampling params) so the same output always hashes to the
same verdict key. See [grading.md](./grading.md).

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
