# A zero-cost gate on every PR

**The problem.** Prompts and templates rot silently. A variable gets renamed and stops substituting; a section separator leaks into the output; a refactor drops the authorization framing. None of it is caught by a type checker, and none of it is worth an LLM bill to catch.

**The shape.** A suite that renders every template your system owns and grades the result with deterministic assertions only. No API key, no network, seconds to run — so it can be a *required* check on every pull request rather than a nightly job people learn to ignore.

## 1. Make your system renderable

The system under test runs in "render" mode: given a template id, produce the rendered text and print it. That is an [`exec` provider](../examples/your-own-system.md#example-13--your-own-system) — one JSON request in, one JSON response out.

```yaml
--8<-- "examples/12-render-health/domarinn.yaml"
```

/// tip | Make the salt move with the build

Rendering is cheap, and a stale render is worse than a re-render — but *omitting* `cache_salt` does not get you that. The key hashes what the provider **sends**, never the bytes of the program sending it, so a rebuilt renderer replays yesterday's output and the gate passes on a template it never rendered.

Pin the salt to something that moves with the build — `cache_salt: "${env:GITHUB_SHA}"` in CI — or run this one gate with `--no-cache`. It costs seconds. See [caching.md](../concepts/caching.md#exec-providers-and-the-provider-salt).

///

## 2. Assert the structural invariants

These are the ones worth having, in rough order of how often they catch something:

| Assertion | Catches |
| --------- | ------- |
| `not-regex: "\\{\\{.*\\}\\}"` | An unsubstituted variable — a render hole. |
| `not-contains` on your separator | Internal metadata or a user/system delimiter leaking through. |
| `length: {min: 1}` | An empty render, usually a missing template. |
| `length: {max: N}` | A runaway loop, before it becomes a token bill. |
| `contains` on required framing | A refactor dropping a policy or authorization line. |

Put the size ceiling in `defaults.assert` so it applies to every case without repetition.

## 3. Make coverage automatic

Listing templates by hand guarantees that the one added last Tuesday has no test. A [generator](../examples/templates-and-test-data.md#example-11--test-generators) enumerates your registry instead, so a new template gets a render test the day it lands:

```yaml
tests:
  - generator:
      command: ["./target/release/my-eval", "generate-tests"]
      timeout_ms: 60000
```

Back it with a unit test in your own codebase asserting the registry and the manifest agree. Otherwise a template can escape the enumeration and the coverage gap is invisible.

## 4. Wire it into CI

```yaml
- name: Prompt render health
  run: domarinn run eval/render-health.yaml
```

That is the whole step. Exit `0` passes, `1` means an assertion failed, `3` means the harness broke. No secrets, so it runs on fork pull requests too — which is exactly where you want a cheap check.

## Verify it actually gates

A gate nobody has seen fail is a gate nobody should trust. Break something on purpose:

```console
$ # temporarily remove a variable from a template, then:
$ domarinn run eval/render-health.yaml
$ echo $?
1
```

If that prints `0`, your assertions are not asserting. [Example 18](../examples/running-and-reporting.md#example-18--a-failing-gate) is a suite that is red by design, kept green-in-CI precisely so the failure path stays exercised.

## See also

- [Example 12](../examples/your-own-system.md#example-12--render-health) — the suite above, runnable.
- [Example 11](../examples/templates-and-test-data.md#example-11--test-generators) — generators in detail.
- [Guide 06](gate-in-ci.md) — the graded layer that runs beside this one.
