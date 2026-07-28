# Contributing to domarinn

Thanks for taking the time to contribute. 🎉

These are guidelines, not laws — use your judgement, and propose changes to them
in a pull request if something here gets in the way.

- [Code of conduct](#code-of-conduct)
- [AI usage policy](#ai-usage-policy)
- [Getting set up](#getting-set-up)
- [The one command that matters](#the-one-command-that-matters)
- [Things that will fail CI](#things-that-will-fail-ci)
- [Submitting an issue](#submitting-an-issue)
- [Naming a pull request](#naming-a-pull-request)
- [Submitting a pull request](#submitting-a-pull-request)

## Code of conduct

See [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md). Found a security issue? Do
**not** open an issue — see [SECURITY.md](./SECURITY.md).

## AI usage policy

domarinn is a harness for evaluating language models, so it would be strange to
ban them. We ask for **disclosure and accountability**, not abstinence:

1. **Disclose it.** If AI wrote or substantially shaped any part of a
   contribution, say so in the pull request. The template has a line for it.
2. **You are the author.** You are responsible for every line you submit. A
   reviewer may ask why any of it is the way it is, and "the model wrote it" is
   not an answer. If you cannot explain a change, do not submit it.
3. **Review before you send.** Read the whole diff yourself first. Untested,
   unread generated code wastes reviewer time, and reviewer time is the scarcest
   thing this project has.
4. **Write your own prose.** Issues, PR descriptions, and review replies should
   be yours. We would rather read three blunt sentences you wrote than six
   polished paragraphs you did not.

## Getting set up

The toolchain is managed by [mise](https://mise.jdx.dev) — Rust, Node, pnpm,
and the lint tools are all pinned in [`.mise/config.toml`](./.mise/config.toml)
and checksummed in `.mise/mise.lock`. You should not need to install any of
them yourself.

```sh
git clone https://github.com/AtvikSecurity/domarinn
cd domarinn
mise install     # installs the pinned toolchain and the git hooks
```

`mise install` also installs the [lefthook](https://lefthook.dev) pre-commit
hooks, which run the shared formatters and lint the workflows with
[zizmor](https://github.com/zizmorcore/zizmor).

Useful tasks (`mise tasks` lists them all):

| Task                 | What it does                                                   |
| -------------------- | -------------------------------------------------------------- |
| `mise run build`     | Build the web UI, then the release binary with the UI embedded |
| `mise run dev`       | Run the server + API on `:8321`                                |
| `mise run test`      | `cargo test --workspace`                                       |
| `mise run lint`      | clippy (as errors) + a formatting check                        |
| `mise run fmt`       | Auto-format                                                    |
| `mise run schema`    | Regenerate `domarinn.schema.json`                              |
| `mise run gen-types` | Regenerate the TypeScript DTOs from the Rust types             |

## The one command that matters

```sh
mise run ci
```

Every CI job is a mise task, and the workflow invokes those tasks — so this runs
byte-for-byte what CI runs. If it passes locally it passes in CI, with one
caveat: `musl-build` needs a musl C toolchain on your host (`musl-tools` on
Debian/Ubuntu) for rusqlite's bundled SQLite.

## Things that will fail CI

Two of the gates catch _generated_ files drifting from their sources, which is
the failure most likely to surprise you:

- **`schema-check`** — you changed a config type but didn't regenerate the JSON
  Schema. Fix: `mise run schema`, then commit `domarinn.schema.json`.
- **`gen-types-check`** — you changed a DTO but didn't regenerate the
  TypeScript types. Fix: `mise run gen-types`, then commit
  `web/src/api/generated/`. Untracked new files count as drift too.

Also: `clippy` runs with `-D warnings`, and the web lint runs with
`--max-warnings=0`. There is no warning budget.

## Submitting an issue

Search the tracker first — it may already exist, possibly with a workaround.

For bugs, please include a **minimal reproduction**: the smallest
`domarinn.yaml` and command that shows the problem. Providers and models vary
enormously, and without a reproduction we usually cannot tell a domarinn bug
from a model being a model. Issues that go stale waiting for one get closed.

## Naming a pull request

**The title of your pull request must be a
[Conventional Commit](https://www.conventionalcommits.org/en/v1.0.0/).** This is
not bookkeeping: the repo squash-merges, so your PR title becomes the commit on
`main`, and [Release Please](https://github.com/googleapis/release-please)
parses those commits to decide the next version and write the changelog. A
mistyped title ships the wrong version number.

```
<type>[(optional scope)][!]: <description>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`,
`ci`, `chore`, `revert`. Scopes in use: `core`, `types`, `protocol`, `cache`,
`server`, `cli`, `web`, `logging`, `config`, `exec`, `docker`, `mise`, `deps`.

A trailing `!` marks a breaking change. While domarinn is pre-`1.0`, `feat` and
`fix` bump the patch version and `!` bumps the minor.

Examples:

- `feat(cli): add --summary-md output`
- `fix(server): reject an SSO login with no email claim`
- `refactor(core)!: rename the assertion trait`

> We do **not** require conventional commits on individual commits inside a PR —
> only the title. If your PR is a single commit, GitHub already uses that
> commit's message as the title, so writing it conventionally gets both for
> free.

## Submitting a pull request

1. Check for an existing PR covering the same thing.
2. For anything large, open an issue first so the design can be discussed before
   you write it. Nobody enjoys rejecting a finished branch.
3. Fork, branch, and make your change. Add tests.
4. Run `mise run ci` and make it pass.
5. Open the PR with a conventional-commit title and fill in the template.

Reviews are open to anyone — please do review each other's work — but only
maintainers can approve and merge.
