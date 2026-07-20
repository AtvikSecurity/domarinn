# Self-hosting measurellm

measurellm is **one static binary**. The `server` subcommand runs the results
API and the embedded web UI; the same binary is the CLI, the eval engine, and
its own container healthcheck. There are **zero runtime dependencies** — no
sidecar database, no external cache, no libc. State is a single SQLite file
under the data directory.

## What the server is (and is not)

- **Single-writer.** Storage is SQLite. That is a deliberate, boring choice: it
  makes backups a file copy and self-hosting a one-liner. It also means the
  service is **one replica**. Do not run two.
- **Stateless except for `/data`.** Everything durable lives in the data
  directory (default `/data`, env `MEASURELLM_DATA_DIR`): the `measurellm.db`
  SQLite database and the shared cache.

## Configuration

| Env var                | Default | Purpose |
|------------------------|---------|---------|
| `MEASURELLM_DATA_DIR`  | `/data` | Directory holding `measurellm.db` and the cache. Mount a volume here. |
| `MEASURELLM_TOKENS`    | (unset) | Auth tokens as comma-separated `name:secret` pairs. When set, run uploads require a bearer token. |
| `MEASURELLM_PUBLIC_URL`| (unset) | Public base URL used in share links and absolute URLs behind a proxy. No trailing slash, no path prefix. |

The server listens on **`0.0.0.0:8321`** (`--port` to change). Health is exposed
at `/health` and `/api/v1/health`; the container `HEALTHCHECK` runs
`measurellm healthcheck`, which probes the server from inside the container (the
distroless image has no shell or curl).

## Docker

```sh
docker run -d --name measurellm \
  -p 8321:8321 \
  -v measurellm-data:/data \
  -e MEASURELLM_TOKENS="ci:CHANGE_ME" \
  -e MEASURELLM_PUBLIC_URL="https://measurellm.example.com" \
  ghcr.io/perfectra1n/measurellm:latest
```

The image is built `FROM gcr.io/distroless/static-debian12:nonroot`: it runs as
a non-root user, contains only the binary, and has nothing to patch but the
binary itself.

## Docker Compose

Use the checked-in [`docker-compose.yml`](../docker-compose.yml):

```sh
docker compose up -d
# UI + API on http://localhost:8321
```

Replace the placeholder token secrets before exposing the service. Prefer
injecting `MEASURELLM_TOKENS` from a real secret store rather than committing it.

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
                secretKeyRef: { name: measurellm-tokens, key: tokens }
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

## Reverse proxies and `X-Forwarded-*`

Only trust `X-Forwarded-For` / `X-Forwarded-Proto` / `X-Forwarded-Host` **when
you have configured measurellm to sit behind a proxy** and the proxy actually
sets them. An untrusted client can forge these headers, so do not enable
forwarded-header trust for a directly-exposed instance. Set `MEASURELLM_PUBLIC_URL`
so the app builds correct absolute (share) URLs regardless of proxy quirks.

## Backups

State is one file: `${MEASURELLM_DATA_DIR}/measurellm.db`.

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
first.
