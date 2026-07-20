//! Ensure the embedded web asset folder exists at compile time.
//!
//! `routes.rs` embeds `../../web/dist` via `rust-embed`, whose derive macro reads
//! that folder when the crate is compiled. A plain `cargo build` (or `test` /
//! `clippy` / a CI job) that has not run `pnpm -C web build` first would have no
//! `web/dist`, and the macro would generate no methods (a confusing `E0599: no
//! function 'get'`). To keep every build mode working, create a minimal
//! placeholder `index.html` when the real UI has not been built. A real
//! `pnpm build` output overwrites it and is embedded instead.

use std::path::Path;

const PLACEHOLDER: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>measurellm</title></head>\
<body style=\"font-family:system-ui;max-width:40rem;margin:4rem auto;padding:0 1rem\">\
<h1>measurellm</h1><p>The web UI was not built into this binary. \
Run <code>mise run build</code> (or <code>pnpm -C web build</code>) before building the \
server to embed it. The JSON API is available under <code>/api/v1</code>.</p>\
</body></html>";

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dist = Path::new(&manifest)
        .join("..")
        .join("..")
        .join("web")
        .join("dist");
    let index = dist.join("index.html");

    if !index.exists() {
        if let Err(e) = std::fs::create_dir_all(&dist) {
            println!("cargo:warning=could not create {}: {e}", dist.display());
            return;
        }
        if let Err(e) = std::fs::write(&index, PLACEHOLDER) {
            println!("cargo:warning=could not write {}: {e}", index.display());
        }
    }

    // Rebuild if the embedded assets change (e.g. after a real `pnpm build`).
    println!("cargo:rerun-if-changed={}", dist.display());
}
