# Evaluate your own application

**The problem.** You are shipping an application, not a model. It has its own prompt registry, its own client, its own retry logic and its own guardrails. Evaluating the model underneath it measures something adjacent to the product — and passes cleanly while the thing customers touch is broken.

**The shape.** Make your application runnable as a subprocess that speaks a small JSON protocol, and point domarinn at *that*.

## 1. Add an eval entry point to your app

One JSON request on stdin, one JSON response on stdout. That is the whole contract.

```python
--8<-- "examples/13-exec-provider/assistant.py"
```

Reuse your real code path. The value of this scenario comes entirely from the eval exercising the same rendering and client logic production does — a wrapper that reimplements them tests the wrapper.

## 2. Point a suite at it

```yaml
--8<-- "examples/13-exec-provider/domarinn.yaml"
```

/// warning | Three things that surprise people

**`command` paths resolve relative to the suite file's directory**, not the shell's working directory. That is what lets `domarinn run eval/` from the repo root and `domarinn run .` from inside `eval/` behave identically.

**The cache fingerprint is the command, not the program's bytes.** Rebuild your binary and the cache still replays the old answers. Set a provider `cache_salt` when a rebuild should invalidate — see [scenario 05](shared-cache.md#2-salt-at-the-right-granularity).

**There is no flag that swaps the model.** The model is part of argv, therefore part of the fingerprint — which is *why* two models can never collide in the cache. To compare two, declare two providers and scope a run with `--provider`.

///

## 3. Report what you know

Three response fields do disproportionate work:

| Field | Enables |
| ----- | ------- |
| `usage` | `tokens:` assertions, and `cost_usd` existing at all |
| `cost_usd` | a `cost:` budget that actually enforces something |
| `empty_reason` | a refusal or a tool-only turn being distinguished from a blank failure |
| `error.retriable` | retries that fire on rate limits and not on rejected credentials |

Omit them and the corresponding assertions pass while enforcing nothing. See [example 31](../examples.md#example-31--budgets) and [example 19](../examples.md#example-19--errors-and-retries).

## 4. Generate the cases from your registry

If your app owns a catalogue of prompts, tools or policies, enumerate it rather than listing cases by hand — otherwise the thing added last Tuesday has no test:

```yaml
tests:
  - generator:
      command: ["./target/release/my-eval", "generate-tests"]
```

See [example 11](../examples.md#example-11--test-generators). Back it with a test in your own codebase asserting the registry and the manifest agree.

## 5. Guard the thing that silently invalidates everything

Pin the suite's model to the one production actually serves, and **assert it in your own test suite**:

```rust
#[test]
fn the_eval_suite_uses_the_model_production_serves() {
    assert_eq!(eval_suite_model(), production_default_model());
}
```

Without a guard like this, a suite can sit on a model nobody serves — and every pass rate you published from it was measuring something you do not ship. It is a cheap test and it catches a failure that is otherwise invisible for months.

## See also

- [Example 13](../examples.md#example-13--your-own-system) — the suite above.
- [Exec protocol](../protocol.md) — the full contract, in three languages.
- [Scenario 01](render-gate.md) — the free layer to put in front of this one.
