# domarinn-protocol

Wire types for [domarinn](https://github.com/AtvikSecurity/domarinn)'s **exec protocol** — the
JSON contract between domarinn and an external program acting as a provider, an assertion, or a
test generator.

Depends on `serde` and `serde_json`. Nothing else, ever.

## The contract

One-shot. domarinn writes exactly one JSON document to your program's stdin, closes stdin, and
reads exactly one JSON document from stdout. Exit non-zero only if you could not produce a
document at all — a _provider_ failure is reported inside the response, not as an exit code.

```rust
use domarinn_protocol::{ProviderReq, ProviderResp};

fn main() -> std::io::Result<()> {
    let req: ProviderReq = serde_json::from_reader(std::io::stdin())?;
    let answer = my_model.complete(&req.vars);
    let resp = ProviderResp {
        output: serde_json::Value::String(answer),
        ..Default::default()
    };
    serde_json::to_writer(std::io::stdout(), &resp)?;
    Ok(())
}
```

`docs/protocol.md` in the main repository is the normative specification, including the request
and response field tables and the vocabulary for `empty_reason` and `class`.

## Why this crate exists separately from `domarinn-types`

Two crates carry wire shapes, for two different audiences:

- **`domarinn-types`** is the _run document_ — what an eval produced. It is read by tools that
  consume results, so it carries `schemars`, `ts-rs`, `chrono`, `sha2`, and `ulid` to generate a
  JSON Schema and TypeScript definitions.
- **`domarinn-protocol`** is the _exec protocol_ — how a program under test talks to the harness.
  Its audience is someone writing a provider in Rust, who should not inherit a schema generator
  and a TypeScript exporter to serialize four structs.

A single "types" crate would force the second audience to pay the first audience's dependency
bill. Keeping them apart is the whole point; `tests/deps.rs` makes that structural rather than
aspirational.

## Versioning

The crate version tracks domarinn's. The number that governs compatibility is `PROTOCOL_VERSION`,
carried in every request's envelope, and it is **1**.

Fields are added to protocol 1 additively: every optional field is `skip_serializing_if`, so a
program written before a field existed produces a byte-identical document and is parsed
identically. Ignore unknown fields you receive — that is how forward compatibility works here, and
domarinn does the same with yours.

## Installing

Not published to crates.io. Depend on it by tag:

```toml
[dependencies]
domarinn-protocol = { git = "https://github.com/AtvikSecurity/domarinn", tag = "0.3.0" }
```

## License

MIT.
