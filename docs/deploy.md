# Self-hosting measurellm

measurellm is **one static binary**. The `server` subcommand runs the results
API and the embedded web UI; the same binary is the CLI, the eval engine, and
its own container healthcheck. There are **zero runtime dependencies** — no
sidecar database, no external cache, no libc. State is SQLite under the data
directory.

For the API surface, the auth model, and the full environment-variable reference,
see [`./server.md`](./server.md). This page is about *hosting* it.

## What the server is (and is not)

- **Single-writer.** Storage is SQLite. That is a deliberate, boring choice: it
  makes backups a file copy and self-hosting a one-liner. It also means the
  service is **one replica**. Do not run two.
- **Stateless except for `/data`.** Everything durable lives in the data
  directory (default `/data`, env `MEASURELLM_DATA_DIR`): the `measurellm.db`
  SQLite database (runs, users, sessions, API keys, baselines) and `cache.db`
  (the disposable content-addressed cache).

## Configuration

The essentials for hosting are below; [`./server.md`](./server.md#environment-variables)
has the complete table (auth modes, admin bootstrap, cache limits).

| Env var                 | Default | Purpose |
|-------------------------|---------|---------|
| `MEASURELLM_DATA_DIR`   | `/data` | Directory holding `measurellm.db` and `cache.db`. Mount a volume here. |
| `MEASURELLM_TOKENS`     | (unset) | Static bearer tokens as comma-separated `scope:secret` pairs (`read:…,write:…,admin:…`). Setting any flips the default auth mode to `protect-writes` (writes/admin require a token; reads stay open). |
| `MEASURELLM_AUTH_MODE`  | (derived) | Force the mode: `open` \| `protect-writes` \| `closed`. |
| `MEASURELLM_ADMIN_USER` / `MEASURELLM_ADMIN_PASSWORD` | (unset) | Bootstrap a local admin account at startup (see [First run](#first-run-creating-the-admin)). |
| `MEASURELLM_PUBLIC_URL` | (unset) | Public base URL used in share links and absolute URLs behind a proxy. No trailing slash, no path prefix. |

The server listens on **`0.0.0.0:8321`** (`--port` to change). Health is exposed
at `/health` and `/api/v1/health`; the container `HEALTHCHECK` runs
`measurellm healthcheck`, which probes the server from inside the container (the
distroless image has no shell or curl).

## First run: creating the admin

A brand-new instance with no tokens and no accounts comes up in **`open`** mode —
anyone can read and write. To lock it down you create an admin, which flips the
instance to `protect-writes`. Two ways:

- **Bootstrap from the environment (recommended for containers).** Set
  `MEASURELLM_ADMIN_USER` and `MEASURELLM_ADMIN_PASSWORD`. On every startup the
  server idempotently ensures that enabled admin account exists — ideal with a
  secret store. An instance seeded this way comes up already in `protect-writes`.
- **Interactive setup.** Hit the `/setup` page (or `POST /api/v1/auth/setup`)
  once to create the first admin.

Full details, including how the mode is derived and how tokens vs accounts differ,
are in [`./server.md`](./server.md#accounts--auth-model).

## The image

The image is a multi-stage build (see the [`Dockerfile`](../Dockerfile)):

1. **web** — `node:22-alpine` builds the React/Vite UI into `web/dist`.
2. **builder** — `rust:1-alpine` compiles a **static musl** binary with
   `web/dist` embedded via `rust-embed`, so the UI ships inside the binary.
3. **runtime** — `gcr.io/distroless/static-debian12:nonroot`: just the binary on
   a scratch-like base. No shell, no libc, no package manager — **nothing to
   CVE-scan but the binary itself**. It runs as a non-root user, and the binary
   is **its own `HEALTHCHECK`** (`measurellm healthcheck` probes
   `/api/v1/health` from inside the container, since there is no curl/wget).

## Docker

```sh
docker run -d --name measurellm \
  -p 8321:8321 \
  -v measurellm-data:/data \
  -e MEASURELLM_ADMIN_USER=admin \
  -e MEASURELLM_ADMIN_PASSWORD='CHANGE_ME' \
  -e MEASURELLM_TOKENS="write:CHANGE_ME_ci,admin:CHANGE_ME_ops" \
  -e MEASURELLM_PUBLIC_URL="https://measurellm.example.com" \
  ghcr.io/perfectra1n/measurellm:latest
```

State persists in the `measurellm-data` volume mounted at `/data`. Replace the
placeholder secrets and inject them from a real secret store rather than a shell
history.

## Docker Compose

Use the checked-in [`docker-compose.yml`](../docker-compose.yml):

```sh
docker compose up -d
# UI + API on http://localhost:8321
```

A production-shaped compose service that bootstraps an admin and sets the public
URL:

```yaml
services:
  measurellm:
    image: ghcr.io/perfectra1n/measurellm:latest
    container_name: measurellm
    restart: unless-stopped
    ports:
      - "8321:8321"
    environment:
      MEASURELLM_DATA_DIR: /data
      # Bootstrap admin — inject from a secret store, not this file, in prod.
      MEASURELLM_ADMIN_USER: admin
      MEASURELLM_ADMIN_PASSWORD: CHANGE_ME
      # Static tokens for CI/scripts (scope:secret). Optional if you only use
      # accounts + API keys.
      MEASURELLM_TOKENS: "write:CHANGE_ME_ci,admin:CHANGE_ME_ops"
      # Public base URL for share links. No trailing slash, no path prefix.
      MEASURELLM_PUBLIC_URL: "https://measurellm.example.com"
    volumes:
      - measurellm-data:/data
    healthcheck:
      # The binary is its own probe (distroless has no shell/curl).
      test: ["CMD", "/measurellm", "healthcheck"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 10s

volumes:
  measurellm-data:
```

Replace the placeholder secrets before exposing the service. Prefer injecting
`MEASURELLM_ADMIN_PASSWORD` / `MEASURELLM_TOKENS` from a real secret store rather
than committing them.

## Kubernetes

Because SQLite is a single writer, deploy exactly **one replica** with a
**`Recreate`** update strategy (never `RollingUpdate` — two pods must never hold
the database open at once) and a **`ReadWriteOnce` PVC** mounted at `/data`.

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: measurellm
spec:
  replicas: 1                 # single writer — do NOT scale up
  strategy:
    type: Recreate            # tear the old pod down before the new one starts
  selector:
    matchLabels: { app: measurellm }
  template:
    metadata:
      labels: { app: measurellm }
    spec:
      containers:
        - name: measurellm
          image: ghcr.io/perfectra1n/measurellm:latest
          ports:
            - containerPort: 8321
          env:
            - name: MEASURELLM_DATA_DIR
              value: /data
            - name: MEASURELLM_PUBLIC_URL
              value: https://measurellm.example.com
            - name: MEASURELLM_TOKENS
              valueFrom:
                secretKeyRef: { name: measurellm-secrets, key: tokens }
            # Bootstrap admin (idempotent on every start).
            - name: MEASURELLM_ADMIN_USER
              valueFrom:
                secretKeyRef: { name: measurellm-secrets, key: admin-user }
            - name: MEASURELLM_ADMIN_PASSWORD
              valueFrom:
                secretKeyRef: { name: measurellm-secrets, key: admin-password }
          volumeMounts:
            - name: data
              mountPath: /data
          livenessProbe:
            httpGet: { path: /api/v1/health, port: 8321 }
            initialDelaySeconds: 5
          readinessProbe:
            httpGet: { path: /api/v1/health, port: 8321 }
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: measurellm-data
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: measurellm-data
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 10Gi
```

Expose it with a plain `Service` + `Ingress`. Give it a **hostname of its own**
(`measurellm.example.com`), **not a path prefix** — the app assumes it is served
at the web root, and `MEASURELLM_PUBLIC_URL` should be that hostname's URL.

<a id="reverse-proxies"></a>

## Reverse proxies and share links

The cleanest setup is to **set `MEASURELLM_PUBLIC_URL`** to the exact external
base URL (scheme + host, no path prefix). When it is set, the server builds share
links straight from it and does **not** consult any forwarded headers — proxy
quirks can't produce a wrong link.

When `MEASURELLM_PUBLIC_URL` is *unset*, the server derives the base URL for
share links from the request's `Host` header and the `X-Forwarded-Proto` header
(defaulting to `http`). Since a client can forge those headers on a
directly-exposed instance, **set `MEASURELLM_PUBLIC_URL` behind any proxy** and
ensure your proxy sets `X-Forwarded-Proto` correctly if you rely on the fallback.
The app is designed to be served at the web root, so route a whole hostname to it
rather than a sub-path.

## Backups

The backup target is one file: `${MEASURELLM_DATA_DIR}/measurellm.db`. (`cache.db`
is a disposable, regenerable cache — no need to back it up.)

- **Volume snapshots** are the simplest option (snapshot the PVC / Docker
  volume).
- For a **hot copy**, use SQLite's online backup rather than `cp` on a live
  database:

  ```sh
  sqlite3 /data/measurellm.db ".backup '/backups/measurellm-$(date +%F).db'"
  ```

  (Run this from a maintenance container — the distroless runtime image has no
  `sqlite3`.)

Restoring is just putting the file back at `/data/measurellm.db` while the
service is stopped.

## Upgrades

Pull the new image tag and restart the single pod/container (`Recreate` on
Kubernetes handles the ordering). Because the schema migrations run at startup
and there is only ever one writer, upgrades are a stop-start with a backup taken
first. Image tags (`latest`, `{{version}}`, `{{major}}.{{minor}}`) are published
by the release workflow — see [`./ci.md`](./ci.md#releases-releaseyml).
