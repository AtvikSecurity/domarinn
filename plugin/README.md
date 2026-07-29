# domarinn — Claude Code plugin

Gives Claude read-only access to your [domarinn](https://github.com/AtvikSecurity/domarinn) eval
history, plus the skills to author and run eval suites locally.

## Install

```bash
claude plugin marketplace add AtvikSecurity/domarinn
claude plugin install domarinn@domarinn
```

Claude Code prompts for two values on enable:

| Setting      | What it is                                                                                                  |
| ------------ | ----------------------------------------------------------------------------------------------------------- |
| `server_url` | Base URL of your domarinn server, e.g. `https://domarinn.example.com`. Defaults to `http://localhost:8321`. |
| `api_token`  | A read-scoped API key (`domarinn_…`) or static token. Stored in secure storage, not `settings.json`.        |

Create a key under **Settings → API keys** in the web UI, or set
`DOMARINN_TOKENS=read:<token>` on the server.

## Server-side prerequisite

The MCP endpoint is **opt-in**. Start the server with:

```bash
DOMARINN_MCP_ENABLED=true domarinn server
```

Without it the endpoint is not mounted and the plugin's tools return 404. Run `/domarinn:check` to
confirm the connection and diagnose anything that is not working.

## What you get

**8 MCP tools**, read-only, covering every read surface the server has:

| Tool              | Answers                                                             |
| ----------------- | ------------------------------------------------------------------- |
| `find_runs`       | What projects, suites, and runs exist                               |
| `get_run`         | How one run went; which matrix cell is unhealthy                    |
| `list_cases`      | Which cases failed                                                  |
| `get_case`        | Why one case failed — assertions, stop reason, output               |
| `case_history`    | Is it flaky or newly broken                                         |
| `compare_runs`    | Did it get worse between two runs (with McNemar + Wilson intervals) |
| `search`          | Full-text across runs and case content                              |
| `get_server_info` | Instance version, accepted schema versions, cache health            |

**3 prompts**, surfaced as slash commands: `/domarinn:triage`, `/domarinn:summarize`,
`/domarinn:check`.

**2 skills** that load on demand:

- `triage-evals` — reading eval history well: which tool answers which question, how to tell a
  regression from a flake, and how to treat stored model output as untrusted.
- `author-evals` — writing `domarinn.yaml` suites, choosing assertions, the caching model, exit
  codes, and CI wiring.

## Safety

Every tool is read-only: nothing here can start a run, mutate a baseline, or delete anything.

Stored model outputs are returned inside an `<untrusted source="stored_model_output">` fence, with
ANSI escapes and control characters stripped and the closing marker neutralized. In a
security-evaluation suite those outputs are adversarial by design; the skills instruct Claude to
treat everything inside the fence as data to analyze, never as instructions to follow.
