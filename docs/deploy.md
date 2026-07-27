# Self-hosting domarinn

domarinn is **one static binary**. The `server` subcommand runs the results
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
  directory (default `/data`, env `DOMARINN_DATA_DIR`): the `domarinn.db`
  SQLite database (runs, users, sessions, API keys, baselines) and `cache.db`
  (the disposable content-addressed cache).

## Configuration

The essentials for hosting are below; [`./server.md`](./server.md#environment-variables)
has the complete table (auth modes, admin bootstrap, cache limits).

| Env var                 | Default | Purpose |
|-------------------------|---------|---------|
| `DOMARINN_DATA_DIR`   | `/data` | Directory holding `domarinn.db` and `cache.db`. Mount a volume here. |
| `DOMARINN_TOKENS`     | (unset) | Static bearer tokens as comma-separated `scope:secret` pairs (`read:…,write:…,admin:…`). Grants access; never changes the mode. |
| `DOMARINN_AUTH_MODE`  | `closed` | The auth mode: `open` \| `protect-writes` \| `closed`. Unset means `closed` — every call and page requires auth. |
| `DOMARINN_ADMIN_USER` / `DOMARINN_ADMIN_PASSWORD` | (unset) | Bootstrap a local admin account at startup (see [First run](#first-run-creating-the-admin)). |
| `DOMARINN_PUBLIC_URL` | (unset) | Public base URL used in share links and absolute URLs behind a proxy. No trailing slash, no path prefix. |

The server listens on **`0.0.0.0:8321`** (`--port` to change). Health is exposed
at `/health` and `/api/v1/health`; the container `HEALTHCHECK` runs
`domarinn healthcheck`, which probes the server from inside the container (the
distroless image has no shell or curl).

## First run: creating the admin

A brand-new instance comes up **`closed`** — every page and API call requires
auth — with only the bootstrap surface open (health, meta, setup, login) so it
can be claimed. Create the admin one of two ways:

- **Bootstrap from the environment (recommended for containers).** Set
  `DOMARINN_ADMIN_USER` and `DOMARINN_ADMIN_PASSWORD`. On every startup the
  server idempotently ensures that enabled admin account exists — ideal with a
  secret store.
- **Interactive setup.** Hit the `/setup` page (or `POST /api/v1/auth/setup`)
  once to create the first admin; it is a 409 once any user exists.

Full details, including the auth modes and how tokens vs accounts differ,
are in [`./server.md`](./server.md#accounts--auth-model).

## The image

The image is a multi-stage build (see the [`Dockerfile`](../Dockerfile)):

1. **web** — `node:22-alpine` builds the React/Vite UI into `web/dist`.
2. **builder** — `rust:1-alpine` compiles a **static musl** binary with
   `web/dist` embedded via `rust-embed`, so the UI ships inside the binary.
3. **runtime** — `gcr.io/distroless/static-debian12:nonroot`: just the binary on
   a scratch-like base. No shell, no libc, no package manager — **nothing to
   CVE-scan but the binary itself**. It runs as a non-root user, and the binary
   is **its own `HEALTHCHECK`** (`domarinn healthcheck` probes
   `/api/v1/health` from inside the container, since there is no curl/wget).

## Docker

```sh
docker run -d --name domarinn \
  -p 8321:8321 \
  -v domarinn-data:/data \
  -e DOMARINN_ADMIN_USER=admin \
  -e DOMARINN_ADMIN_PASSWORD='CHANGE_ME' \
  -e DOMARINN_TOKENS="write:CHANGE_ME_ci,admin:CHANGE_ME_ops" \
  -e DOMARINN_PUBLIC_URL="https://domarinn.example.com" \
  ghcr.io/atviksecurity/domarinn:rolling
```

State persists in the `domarinn-data` volume mounted at `/data`. Replace the
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
  domarinn:
    image: ghcr.io/atviksecurity/domarinn:rolling
    container_name: domarinn
    restart: unless-stopped
    ports:
      - "8321:8321"
    environment:
      DOMARINN_DATA_DIR: /data
      # Bootstrap admin — inject from a secret store, not this file, in prod.
      DOMARINN_ADMIN_USER: admin
      DOMARINN_ADMIN_PASSWORD: CHANGE_ME
      # Static tokens for CI/scripts (scope:secret). Optional if you only use
      # accounts + API keys.
      DOMARINN_TOKENS: "write:CHANGE_ME_ci,admin:CHANGE_ME_ops"
      # Public base URL for share links. No trailing slash, no path prefix.
      DOMARINN_PUBLIC_URL: "https://domarinn.example.com"
    volumes:
      - domarinn-data:/data
    healthcheck:
      # The binary is its own probe (distroless has no shell/curl).
      test: ["CMD", "/domarinn", "healthcheck"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 10s

volumes:
  domarinn-data:
```

Replace the placeholder secrets before exposing the service. Prefer injecting
`DOMARINN_ADMIN_PASSWORD` / `DOMARINN_TOKENS` from a real secret store rather
than committing them.

## `/data` ownership (bind mounts, existing volumes)

The container runs as the distroless `nonroot` user, **uid 65532** — never root.
The server must be able to create and write its SQLite files in `/data`; if it
cannot, it exits at startup with
`server error: opening sqlite db at /data/domarinn.db: … permission denied`.

- **Named volumes** (everything above) just work: Docker seeds a fresh volume
  with the image's `/data` ownership, which the image sets to uid 65532.
- **Bind mounts** (`-v ./data:/data`) keep the host directory's ownership, which
  is almost never 65532. Either chown the host directory:

  ```sh
  mkdir -p ./data && sudo chown 65532:65532 ./data
  ```

  or run the container as the directory's owner — the binary is fully static
  and uid-agnostic, so any uid works:

  ```yaml
  services:
    domarinn:
      user: "1000:1000"   # match the bind-mounted directory's owner
  ```

- **Volumes created by images older than the ownership fix** are root-owned,
  but Docker re-seeds ownership from the image whenever it mounts an **empty**
  named volume — and a volume from a failed pre-fix deployment is empty, since
  the server could never create its files. Upgrading the image therefore fixes
  these volumes automatically. Only a root-owned volume that already **contains
  files** needs a one-time repair:

  ```sh
  docker run --rm -v domarinn-data:/data busybox chown -R 65532:65532 /data
  ```

## Kubernetes

Because SQLite is a single writer, deploy exactly **one replica** with a
**`Recreate`** update strategy (never `RollingUpdate` — two pods must never hold
the database open at once) and a **`ReadWriteOnce` PVC** mounted at `/data`.

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: domarinn
spec:
  replicas: 1                 # single writer — do NOT scale up
  strategy:
    type: Recreate            # tear the old pod down before the new one starts
  selector:
    matchLabels: { app: domarinn }
  template:
    metadata:
      labels: { app: domarinn }
    spec:
      # Fresh PVC filesystems are root-owned; fsGroup makes the kubelet chown
      # the volume to the pod's group so the nonroot server (uid 65532) can
      # create its SQLite files. Docker's volume-ownership seeding does not
      # exist in Kubernetes, so this is required, not belt-and-suspenders.
      securityContext:
        fsGroup: 65532
      containers:
        - name: domarinn
          image: ghcr.io/atviksecurity/domarinn:rolling
          ports:
            - containerPort: 8321
          env:
            - name: DOMARINN_DATA_DIR
              value: /data
            - name: DOMARINN_PUBLIC_URL
              value: https://domarinn.example.com
            - name: DOMARINN_TOKENS
              valueFrom:
                secretKeyRef: { name: domarinn-secrets, key: tokens }
            # Bootstrap admin (idempotent on every start).
            - name: DOMARINN_ADMIN_USER
              valueFrom:
                secretKeyRef: { name: domarinn-secrets, key: admin-user }
            - name: DOMARINN_ADMIN_PASSWORD
              valueFrom:
                secretKeyRef: { name: domarinn-secrets, key: admin-password }
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
            claimName: domarinn-data
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: domarinn-data
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 10Gi
```

Expose it with a plain `Service` + `Ingress`. Give it a **hostname of its own**
(`domarinn.example.com`), **not a path prefix** — the app assumes it is served
at the web root, and `DOMARINN_PUBLIC_URL` should be that hostname's URL.

<a id="reverse-proxies"></a>

## Reverse proxies and share links

The cleanest setup is to **set `DOMARINN_PUBLIC_URL`** to the exact external
base URL (scheme + host, no path prefix). When it is set, the server builds share
links straight from it and does **not** consult any forwarded headers — proxy
quirks can't produce a wrong link.

When `DOMARINN_PUBLIC_URL` is *unset*, the server derives the base URL for
share links from the request's `Host` header and the `X-Forwarded-Proto` header
(defaulting to `http`). Since a client can forge those headers on a
directly-exposed instance, **set `DOMARINN_PUBLIC_URL` behind any proxy** and
ensure your proxy sets `X-Forwarded-Proto` correctly if you rely on the fallback.
The app is designed to be served at the web root, so route a whole hostname to it
rather than a sub-path.

## Backups

The backup target is one file: `${DOMARINN_DATA_DIR}/domarinn.db`. (`cache.db`
is a disposable, regenerable cache — no need to back it up.)

- **Volume snapshots** are the simplest option (snapshot the PVC / Docker
  volume).
- For a **hot copy**, use SQLite's online backup rather than `cp` on a live
  database:

  ```sh
  sqlite3 /data/domarinn.db ".backup '/backups/domarinn-$(date +%F).db'"
  ```

  (Run this from a maintenance container — the distroless runtime image has no
  `sqlite3`.)

Restoring is just putting the file back at `/data/domarinn.db` while the
service is stopped.

## Upgrades

Pull the new image tag and restart the single pod/container (`Recreate` on
Kubernetes handles the ordering). Because the schema migrations run at startup
and there is only ever one writer, upgrades are a stop-start with a backup taken
first.

Published tags are `rolling` (tracks `main`), plus `{{version}}`,
`{{major}}.{{minor}}` and `{{major}}` for each release. There is deliberately
**no `latest`** — pin a version, or track `rolling` if you want the tip of main.
See [`./ci.md`](./ci.md#container-image-dockeryml).
