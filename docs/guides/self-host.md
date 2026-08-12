<!-- Ownership: operating semantics (auth model, credential kinds, env vars,
storage, logging) live in reference/server.md. Procedures (getting it running:
Docker, compose, Kubernetes, proxies, backups, upgrades, first admin) live in
guides/self-host.md. The env-var table exists ONLY in reference/server.md. -->

# Self-hosting domarinn

domarinn is **one static binary**. The `server` subcommand runs the results API and the embedded web UI; the same binary is the CLI, the eval engine, and its own container healthcheck. There are **zero runtime dependencies** — no sidecar database, no external cache, no libc. State is SQLite under the data directory by default, or [Postgres](#postgres) when you opt in.

For the API surface, the auth model, and the full environment-variable reference, see [`../reference/server.md`](../reference/server.md). This page is about *hosting* it.

## What the server is (and is not)

- **Single-writer by default.** Storage is SQLite unless you opt into [Postgres](#postgres). SQLite is a deliberate, boring choice: it makes backups a file copy and self-hosting a one-liner. It also means the service is **one replica** — do not run two against the same data dir. When you need more than one, that is what the Postgres backend is for.
- **Stateless except for `/data`.** Everything durable lives in the data directory (default `/data`, env `DOMARINN_DATA_DIR`): the `domarinn.db` SQLite database (runs, users, sessions, API keys, baselines) and `cache.db` (the disposable content-addressed cache). Under Postgres, everything durable moves into the database; `/data` stays mounted for the local cache tier.

## Configuration

The complete environment-variable table — auth modes, admin bootstrap, SSO, cache limits, MCP,
logging — lives in [`../reference/server.md`](../reference/server.md#environment-variables); it is
the sole owner of that table. The two you always set for hosting are `DOMARINN_DATA_DIR` (mount a
volume there) and `DOMARINN_PUBLIC_URL` (the public base URL, no trailing slash, no path prefix).

The server listens on **`0.0.0.0:8321`** (`--port` to change). Health is exposed at `/health` and `/api/v1/health`; the container `HEALTHCHECK` runs `domarinn healthcheck`, which probes the server from inside the container (the distroless image has no shell or curl).

## First run: creating the admin

Until an admin account exists, `GET /api/v1/meta` reports `"setup_required": true`. A brand-new
instance comes up **`closed`** — every page and API call requires auth — with only the bootstrap
surface open (health, meta, setup, login) so it can be claimed. There are **two** ways to create
that first admin:

**A. Interactive setup (via the UI or the API).** `POST /api/v1/auth/setup` with `{username, password}` creates the first admin and returns a session token. This endpoint is open **only while zero users exist**; afterwards it is a `409`. The web UI's `/setup` page drives exactly this call.

```sh
curl -sX POST http://localhost:8321/api/v1/auth/setup \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"correct horse battery staple"}'
# 201 { "token": "mses_...", "user": { "id": "...", "username": "admin", "role": "admin", ... } }
```

**B. Bootstrap from the environment (recommended for containers).** Set `DOMARINN_ADMIN_USER` and `DOMARINN_ADMIN_PASSWORD`. On every startup the server **idempotently** ensures that account exists as an enabled admin, creating it if missing and updating the password if it changed. This is the right choice for containers and Kubernetes — declare the admin in your secret store and the instance self-seeds.

> An instance seeded this way needs no interactive setup: `setup_required` is
> already `false` on first boot and the seeded admin can log straight in.

Full details, including the auth modes and how tokens vs accounts differ, are in [`../reference/server.md`](../reference/server.md#accounts--auth-model).

## The image

The image is a multi-stage build (see the [`Dockerfile`](https://github.com/AtvikSecurity/domarinn/blob/main/Dockerfile)):

1. **web** — `node:22-alpine` builds the React/Vite UI into `web/dist`.
2. **builder** — `rust:1-alpine` compiles a **static musl** binary with `web/dist` embedded via `rust-embed`, so the UI ships inside the binary.
3. **runtime** — `gcr.io/distroless/static-debian12:nonroot`: just the binary on a scratch-like base. No shell, no libc, no package manager — **nothing to CVE-scan but the binary itself**. It runs as a non-root user, and the binary is **its own `HEALTHCHECK`** (`domarinn healthcheck` probes `/api/v1/health` from inside the container, since there is no curl/wget).

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

State persists in the `domarinn-data` volume mounted at `/data`. Replace the placeholder secrets and inject them from a real secret store rather than a shell history.

## Docker Compose

Use the checked-in [`docker-compose.yml`](https://github.com/AtvikSecurity/domarinn/blob/main/docker-compose.yml):

```sh
docker compose up -d
# UI + API on http://localhost:8321
```

It reads per-deployment settings from a `.env` beside it, so the committed file
stays generic and your host's address never becomes anyone else's default:

```sh
# .env
DOMARINN_PUBLIC_URL=https://domarinn.example.com
```

Reaching the server over plain HTTP — a LAN box, a local trial — also needs the
session cookie's `Secure` flag off, or the browser refuses to store it and no
one can stay signed in. Leave it unset anywhere reachable from the internet:

```sh
# .env, for an HTTP-only deployment
DOMARINN_PUBLIC_URL=http://192.168.1.10:8321
DOMARINN_COOKIE_SECURE=false
```

A production-shaped compose service that bootstraps an admin and sets the public URL:

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

Replace the placeholder secrets before exposing the service. Prefer injecting `DOMARINN_ADMIN_PASSWORD` / `DOMARINN_TOKENS` from a real secret store rather than committing them.

## `/data` ownership (bind mounts, existing volumes)

The container runs as the distroless `nonroot` user, **uid 65532** — never root. The server must be able to create and write its SQLite files in `/data`; if it cannot, it exits at startup with `server error: opening sqlite db at /data/domarinn.db: … permission denied`.

- **Named volumes** (everything above) just work: Docker seeds a fresh volume with the image's `/data` ownership, which the image sets to uid 65532.
- **Bind mounts** (`-v ./data:/data`) keep the host directory's ownership, which is almost never 65532. Either chown the host directory:

  ```sh
  mkdir -p ./data && sudo chown 65532:65532 ./data
  ```

  or run the container as the directory's owner — the binary is fully static and uid-agnostic, so any uid works:

  ```yaml
  services:
    domarinn:
      user: "1000:1000"   # match the bind-mounted directory's owner
  ```

- **Volumes created by images older than the ownership fix** are root-owned, but Docker re-seeds ownership from the image whenever it mounts an **empty** named volume — and a volume from a failed pre-fix deployment is empty, since the server could never create its files. Upgrading the image therefore fixes these volumes automatically. Only a root-owned volume that already **contains files** needs a one-time repair:

  ```sh
  docker run --rm -v domarinn-data:/data busybox chown -R 65532:65532 /data
  ```

## Kubernetes

Under the default SQLite storage, because SQLite is a single writer, deploy exactly **one replica** with a **`Recreate`** update strategy (never `RollingUpdate` — two pods must never hold the database open at once) and a **`ReadWriteOnce` PVC** mounted at `/data`. The [Postgres backend](#postgres) lifts both constraints: with `DOMARINN_DATABASE_URL` set, several replicas may share one database and `RollingUpdate` is safe.

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: domarinn
spec:
  replicas: 1                 # single writer (SQLite) — do NOT scale up
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

Expose it with a plain `Service` + `Ingress`. Give it a **hostname of its own** (`domarinn.example.com`), **not a path prefix** — the app assumes it is served at the web root, and `DOMARINN_PUBLIC_URL` should be that hostname's URL.

## Postgres

Setting `DOMARINN_DATABASE_URL` moves **all durable state** — runs, users, sessions, API keys, baselines, and the shared cache — into one Postgres database. What that changes semantically (multi-replica support, TLS, collation, full-text-search parity) is in [`../reference/server.md`](../reference/server.md#storage), the sole owner of those details; this section is about hosting it.

**When to choose it:**

- **A shared team server** whose history should live in a database you already operate, monitor, and back up.
- **More than one replica.** Several domarinn replicas can share one database — startup migrations serialize under an advisory lock, and the per-replica background tasks are idempotent. SQLite can never offer this.
- **Kubernetes**, where a Postgres service is more natural than a `ReadWriteOnce` PVC — run `replicas: 2+` with a plain `RollingUpdate`, and the PVC constraints above stop applying.
- **A managed database.** The driver is pure Rust with TLS built in (no libpq to install), so a managed Postgres needs nothing but the URL — put `sslmode=require` in it.

Keep `/data` mounted either way: it still holds the local cache tier, and (until you migrate) the SQLite files.

### Compose with a Postgres service

```yaml
services:
  domarinn:
    image: ghcr.io/atviksecurity/domarinn:rolling
    restart: unless-stopped
    ports:
      - "8321:8321"
    environment:
      DOMARINN_DATA_DIR: /data
      # All durable state lives in Postgres; /data keeps the local cache tier.
      DOMARINN_DATABASE_URL: "postgres://domarinn:CHANGE_ME_db_password@postgres:5432/domarinn"
      DOMARINN_PUBLIC_URL: "https://domarinn.example.com"
    volumes:
      - domarinn-data:/data
    depends_on:
      postgres:
        condition: service_healthy
    healthcheck:
      # The binary is its own probe (distroless has no shell/curl).
      test: ["CMD", "/domarinn", "healthcheck"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 10s

  postgres:
    image: postgres:17-alpine
    restart: unless-stopped
    environment:
      POSTGRES_USER: domarinn
      POSTGRES_PASSWORD: CHANGE_ME_db_password
      POSTGRES_DB: domarinn
      # The C locale sorts text bytewise, matching SQLite's ordering exactly.
      # Recommended, not required — see ../reference/server.md#postgres.
      POSTGRES_INITDB_ARGS: "--locale=C --encoding=UTF8"
    volumes:
      - postgres-data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U domarinn -d domarinn"]
      interval: 10s
      timeout: 5s
      retries: 5

volumes:
  domarinn-data:
  postgres-data:
```

Replace the placeholder password (both occurrences — they must match) and inject it from a secret store rather than committing it. The checked-in [`docker-compose.yml`](https://github.com/AtvikSecurity/domarinn/blob/main/docker-compose.yml) carries the same Postgres service as an opt-in variant of the default SQLite setup.

### Migrating an existing SQLite deployment

`domarinn migrate-db` copies a SQLite data dir into an **empty** Postgres database (it refuses a non-empty one), verifies per-table row counts, and prints a summary — see [`../reference/cli.md`](../reference/cli.md#domarinn-migrate-db---data-dir-dir---database-url-url). A fresh install skips this entirely: just start the server with `DOMARINN_DATABASE_URL` set and it creates its schema.

1. **Stop the server.** SQLite is a single writer, and the migration must be the only thing holding the database open.
2. **Run the migration** (the flags fall back to `DOMARINN_DATA_DIR` / `DOMARINN_DATABASE_URL`):

    ```sh
    domarinn migrate-db --data-dir /data --database-url "postgres://domarinn:…@postgres:5432/domarinn"
    ```

3. **Start the server with `DOMARINN_DATABASE_URL` set.** The SQLite files are left in place as a rollback: point the server back at the data dir (by unsetting the URL) and it runs as before — minus whatever was written to Postgres in the meantime.

### Backups under Postgres

`pg_dump` (or your managed database's snapshots) replaces copying the data dir — the [Backups](#backups) section below is about SQLite. `/data` still holds the local cache tier, which is disposable and needs no backup.

<a id="reverse-proxies"></a>

## Reverse proxies and share links

The cleanest setup is to **set `DOMARINN_PUBLIC_URL`** to the exact external base URL (scheme + host, no path prefix). When it is set, the server builds share links straight from it and does **not** consult any forwarded headers — proxy quirks can't produce a wrong link.

When `DOMARINN_PUBLIC_URL` is *unset*, the server derives the base URL for share links from the request's `Host` header and the `X-Forwarded-Proto` header (defaulting to `http`). Since a client can forge those headers on a directly-exposed instance, **set `DOMARINN_PUBLIC_URL` behind any proxy** and ensure your proxy sets `X-Forwarded-Proto` correctly if you rely on the fallback. The app is designed to be served at the web root, so route a whole hostname to it rather than a sub-path.

## Backups

Under the default SQLite storage, the backup target is one file: `${DOMARINN_DATA_DIR}/domarinn.db`. (`cache.db` is a disposable, regenerable cache — no need to back it up.) Under Postgres, back up the database instead — see [Backups under Postgres](#backups-under-postgres).

- **Volume snapshots** are the simplest option (snapshot the PVC / Docker volume).
- For a **hot copy**, use SQLite's online backup rather than `cp` on a live database:

  ```sh
  sqlite3 /data/domarinn.db ".backup '/backups/domarinn-$(date +%F).db'"
  ```

  (Run this from a maintenance container — the distroless runtime image has no `sqlite3`.)

Restoring is just putting the file back at `/data/domarinn.db` while the service is stopped.

## Upgrades

Pull the new image tag and restart the single pod/container (`Recreate` on Kubernetes handles the ordering). Because the schema migrations run at startup and — on SQLite — there is only ever one writer, upgrades are a stop-start with a backup taken first. Under [Postgres](#postgres), a rolling upgrade is safe: replicas racing through startup serialize their migrations under an advisory lock.

Published tags are `rolling` (tracks `main`), plus `{{version}}`, `{{major}}.{{minor}}` and `{{major}}` for each release. There is deliberately **no `latest`** — pin a version, or track `rolling` if you want the tip of main. See [`CONTRIBUTING.md`](https://github.com/AtvikSecurity/domarinn/blob/main/CONTRIBUTING.md#container-image-dockeryml).
