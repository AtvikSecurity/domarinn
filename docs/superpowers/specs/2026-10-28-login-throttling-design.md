# Login throttling that cannot lock out the real user

Status: approved direction (2026-10-28). Replaces the in-memory `LoginThrottle` in `crates/domarinn-server/src/accounts/throttle.rs`.

## Why

Password login is throttled today by an in-memory, per-process map of fixed windows keyed on (client address, username). It has three structural problems, and each patch on top of it so far has traded one denial of service for another:

1. **Behind an ingress the server never sees the client.** The peer address is the ingress controller, so the (address, username) key degrades to the username alone, and five bad guesses from anyone keep the real owner out of password login. There is no way to tell the server which hop to trust.
2. **Replicas don't share state.** With N replicas on Postgres an attacker gets N times the guesses, and a lockout on one pod does not exist on the others.
3. **A lockout punishes the victim, not the attacker.** Any scheme that refuses an account's correct password because of failures it didn't make is an account-lockout DoS. The last fix removed the per-client budget for exactly this reason, which left one-password-across-many-usernames spraying unbounded.

The real deployments are both shapes, and both must be first class: a single container reached directly (SQLite or Postgres), and several replicas behind an ingress sharing Postgres (the maintainer's own setup).

## Goals

- Bound online guessing against any one account, in line with NIST SP 800-63B §5.2.2 (well under 100 consecutive failures per account) and the OWASP Authentication Cheat Sheet.
- An attacker must never be able to stop the legitimate user from signing in from a browser they have signed in from before, however hard the account is hammered. This is the OWASP "device cookie" scheme ("Slow Down Online Guessing Attacks with Device Cookies").
- Bound what an unauthenticated client can cost the server (argon2 CPU and memory) independently of which credentials it sends.
- Identical behaviour on single-node SQLite and multi-replica Postgres; counters that bound guessing are shared through the database.
- Constant-shape responses: status, body and timing must not reveal whether a username exists.

## Non-goals

- CAPTCHA, email/MFA flows, or anything needing outside services.
- Protecting API-key or SSO authentication; only the password paths (`POST /auth/login`, `POST /auth/setup`, `POST /me/password`) are in scope. Only the web UI uses password login; the CLI and CI use API keys.
- A metrics endpoint (none exists); tracing logs only.

## Anti-goals (a failure even if it "works")

- Any path where failures the account owner didn't make cause their correct password to be refused **on a browser that holds a valid device cookie**.
- Trusting `X-Forwarded-For` from a peer that isn't configured as a proxy (that hands every guess a fresh identity).
- Per-replica guessing budgets on a shared Postgres deployment.
- Throttle storage an attacker can grow without bound.

## Design

### 1. Real client address (`DOMARINN_TRUSTED_PROXIES`)

New setting, comma-separated IPs/CIDRs, empty by default. A middleware resolves a `ClientIp` once per request and puts it in the request extensions for every handler to use (login, the existing MCP limiter, request logging):

- If the TCP peer is not in the trusted set, the client is the peer. Forwarding headers are ignored.
- If it is, walk `X-Forwarded-For` from right to left, skipping addresses that are themselves trusted; the first untrusted address is the client. If the header is missing, malformed, or every hop is trusted, fall back to the peer. (`X-Forwarded-For` only; `Forwarded` / RFC 7239 is not needed by ingress-nginx or Traefik and is left out.)
- Log one warning per process if a request from a private-range peer carries `X-Forwarded-For` while no trusted proxies are configured, naming the setting: that is the misconfiguration that collapses every client into one.
- Wherever a client address keys a limit, group IPv6 by /64 and treat IPv4-mapped addresses as IPv4 (as `peer_bucket()` does today).

### 2. Per-client cost limit (in memory, per replica)

A token bucket per client address (/64 for IPv6) on the unauthenticated password endpoints, checked **before** any argon2 work: burst 10, refilling at 10 per minute. Over the limit is a 429 with `Retry-After`, whatever the credentials. Refusing a correct password here is acceptable because it is per client, not per account, and no person types ten passwords a minute; it is what makes spraying usernames from one address cost the attacker time instead of costing the server CPU. It stays in memory per replica on purpose: it protects that replica's CPU, and each replica's CPU is exactly what it should account for. Bound the map (evicting idle full buckets is harmless here, since a full bucket is the default state). The existing argon2 semaphore (permit moved into the blocking task) stays as the global per-replica ceiling.

### 3. Per-account guessing limit with device cookies (in the database)

**Device cookie.** On every successful password login, set `domarinn_device`: `HttpOnly`, `SameSite=Strict`, `Path=/api/v1/auth`, `Secure` following the existing session-cookie rule (`DOMARINN_COOKIE_SECURE` / `DOMARINN_PUBLIC_URL`), `Max-Age` one year. Value `v1.<user_id>.<nonce>.<mac>`, where the MAC is HMAC-SHA256 over the rest with a server secret. The secret is generated on first start and stored in the database (new table on both backends) so every replica accepts every replica's cookies. The cookie only means "this browser has signed in as this user before". It never authenticates anything, survives logout (logout must not clear it), and a cookie whose `user_id` doesn't match the account being signed into counts as absent.

**The login decision**, after the per-client limit and once the username is parsed:

- **Trusted path** (valid device cookie for this account): the counter is per device (`device:<nonce>`). If that device has 10 failures in the window it is locked for the window. Other devices and the untrusted path are unaffected.
- **Untrusted path** (no valid cookie): the counter is per account across all untrusted clients (`account:<normalized username>`). After 10 failures in a 15-minute window, untrusted sign-in to that account is locked. The first lock lasts 5 minutes; each further lock within 24 hours doubles it, capped at 1 hour. While locked, untrusted attempts get 429 **without verifying the password**: the account is under attack, and the owner gets in from a known browser, through SSO, or via an admin password reset.
- Usernames that don't exist are tracked exactly like ones that do (keyed by the normalized username string, verified against the dummy hash), so neither lockout behaviour nor timing reveals which accounts exist.
- A success clears the counter of the path it came through, and always issues or refreshes the device cookie.
- An admin password reset clears the account's untrusted lockout.

The 429 says what happened and when to retry, and points at the way out, e.g. "too many failed sign-in attempts for this account from unrecognised browsers; try again in 12 minutes, or sign in from a browser you've used before". It is the same sentence whether or not the account exists. The web UI already shows server messages through `apiErrorMessage`.

**`POST /me/password`** (caller already authenticated): a per-account counter (`pwchange:<user_id>`) in the same store, 5 failures per 15 minutes, then 429. It can only ever affect the caller.

**`POST /auth/setup`**: per-client cost limit only.

**Storage.** A `login_attempts` table on both backends: key, failure count, window start, `locked_until`, lockout count and its window start. Updates are a single atomic upsert that returns the new state (Postgres `INSERT … ON CONFLICT … DO UPDATE … RETURNING`; SQLite the same with `RETURNING`), so concurrent replicas can't race past a limit. The hourly retention task prunes rows whose windows and locks have all lapsed. Row growth is bounded in practice by the per-client cost limit (an address can create at most about 10 rows a minute) together with pruning. Document that a botnet can still add rows at its aggregate rate until they lapse; that is the accepted cost.

### 4. Removed

The in-memory `LoginThrottle`, `MAX_THROTTLE_KEYS` and the fail-closed-when-full logic all go. Keep `peer_bucket()`'s grouping (moved to wherever the client-address code lives) and the dummy-hash constant-time verify.

## Decided by the maintainer

- Support both deployment shapes properly; the maintainer's own is several replicas behind an ingress on Postgres.
- The device-cookie approach, a trusted-proxy setting, shared state in the database, and progressive lockouts rather than one fixed window.

## Calls made by the designer (check these)

- While an account is under attack, untrusted browsers stay locked out even with the correct password; there is no other bypass. Recovery is a known browser, SSO, or an admin reset. (The maintainer was asked and left this open.)
- Thresholds: 10 untrusted failures per 15 minutes; locks of 5 minutes doubling to a 1-hour cap within 24 hours; 10 per device; 5 for password change; per-client burst 10 refilling at 10 a minute.
- The device cookie lives for a year and survives logout and password changes. It is not invalidated by a password change because it grants no access, only exemption from other people's failures.
- `X-Forwarded-For` only, not `Forwarded`.
- The per-client limiter is in memory per replica; only the per-account and per-device counters are shared.

## Left to the builder

The module layout, the exact table schema and migration mechanics on each backend, the HMAC crate choice (prefer one already in the dependency tree), how the middleware exposes `ClientIp`, and the wording of logs and docs.

## Docs to update

`docs/reference/server.md` (the auth throttling section, a new `DOMARINN_TRUSTED_PROXIES` row, the device cookie), `docs/guides/self-host.md` (the ingress example sets `DOMARINN_TRUSTED_PROXIES` and explains why), `docs/reference/rest-api.md` (the login 429 and its body).

## Testing that proves it

- Client address: a spoofed `X-Forwarded-For` from an untrusted peer is ignored; a multi-hop chain resolves to the first untrusted address from the right; malformed headers fall back; IPv6 grouping works.
- Device cookie: sign and verify, tampering rejected, a cookie for another `user_id` treated as absent, survives logout.
- The anti-goal, directly: an attacker drives an account into untrusted lockout, then the owner's browser with a device cookie signs in with the correct password and succeeds, while an untrusted correct password gets 429.
- An unknown username shows the same statuses, body and lockout progression as a real one.
- Progressive lock durations and their cap.
- **Two app instances over one Postgres share counters**: failures split across instances lock the account on both.
- The per-client limit refuses before any argon2 runs (assert the verifier is not reached).
- An end-to-end run through a real listening server with `DOMARINN_TRUSTED_PROXIES` set and `X-Forwarded-For` sent.
- Every storage-backed test runs on both SQLite and Postgres (`mise run test-pg`, Docker).
