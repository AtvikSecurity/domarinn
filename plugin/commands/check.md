---
description: Check that the domarinn MCP connection is working, and report what it sees
---

Verify this plugin's connection to the domarinn server and report what it can reach.

1. Call `get_server_info` with `include: ["cache"]`. Report the server version, its auth mode,
   which result-schema versions it accepts, and cache health.
2. Call `find_runs` with `group_by: "project"` to show what history exists.
3. If either call fails, diagnose it rather than just relaying the error:
   - **404** — the MCP endpoint is not mounted. The server needs `DOMARINN_MCP_ENABLED=true`.
   - **401** — the token is missing, wrong, or lacks `read` scope. Re-run `/plugin` to set
     `api_token`, or check `DOMARINN_TOKENS` on the server.
   - **403 with "origin not allowed"** — the server's origin allowlist rejected the request. Set
     `DOMARINN_PUBLIC_URL`, or add the origin to `DOMARINN_MCP_ALLOWED_ORIGINS`.
   - **Connection refused** — `server_url` is wrong or the server is down.

Finish with a one-line verdict: connected and what is visible, or not connected and the single
next step.
