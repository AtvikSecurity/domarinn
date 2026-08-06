# Changelog

## [0.10.0](https://github.com/AtvikSecurity/domarinn/compare/0.9.0...0.10.0) (2026-08-06)


### ⚠ BREAKING CHANGES

* **cache:** do not store empty provider outputs unless they are reproducible ([#79](https://github.com/AtvikSecurity/domarinn/issues/79))

### Features

* **cache:** targeted eviction by empty reason, age window, model and kind ([#80](https://github.com/AtvikSecurity/domarinn/issues/80)) ([35bbeed](https://github.com/AtvikSecurity/domarinn/commit/35bbeeda6fbb44d4750212b14c3caba03c127f5a))
* **providers:** fall back to another provider on a refusal or a call failure ([35bbeed](https://github.com/AtvikSecurity/domarinn/commit/35bbeeda6fbb44d4750212b14c3caba03c127f5a))


### Bug Fixes

* **cache:** do not store empty provider outputs unless they are reproducible ([#79](https://github.com/AtvikSecurity/domarinn/issues/79)) ([35bbeed](https://github.com/AtvikSecurity/domarinn/commit/35bbeeda6fbb44d4750212b14c3caba03c127f5a))

## [0.9.0](https://github.com/AtvikSecurity/domarinn/compare/0.8.0...0.9.0) (2026-08-04)


### ⚠ BREAKING CHANGES

* **core:** `VendorCall` gains a `keyed_body` field. Every in-tree constructor is updated; an out-of-tree one must supply it. Cache keys are unchanged for any provider that declares no `request.body` — the overlay is absent, so both bodies are the document that was keyed before.

### Bug Fixes

* **core:** apply the `request.body` overlay on the judge and embeddings paths ([#78](https://github.com/AtvikSecurity/domarinn/issues/78)) ([ba479d6](https://github.com/AtvikSecurity/domarinn/commit/ba479d61a199cf21841c0cc6257efa152056797f))
* **runs:** hide every fully cached run, whatever its verdict ([f684322](https://github.com/AtvikSecurity/domarinn/commit/f684322331363084032adabe72286686ef543e2b))


### Documentation

* **cache:** correct why a replay's verdict does not save it ([77e9662](https://github.com/AtvikSecurity/domarinn/commit/77e966249dd8f211b1e86c45c5f3a49fee345f60))

## [0.8.0](https://github.com/AtvikSecurity/domarinn/compare/0.7.1...0.8.0) (2026-08-03)


### ⚠ BREAKING CHANGES

* **core:** `base_url` is no longer part of the cache key for the vendor providers. A gateway and a direct connection to the same API now share entries instead of paying for the same answers twice — the same call `cache_key` already made about the model a vendor reports having served. The address is recorded on each entry and a hit from a different one warns, exactly as `program_digest` reports a rebuilt `exec` program; `cache_salt` (new on these variants) separates two endpoints outright when a warning is not enough.

### Features

* **core:** customize provider requests via `request:`, and stop keying base_url ([#65](https://github.com/AtvikSecurity/domarinn/issues/65)) ([56f9298](https://github.com/AtvikSecurity/domarinn/commit/56f92986328da0f3cc9a4b752c0fb818ced8a017))

## [0.7.1](https://github.com/AtvikSecurity/domarinn/compare/0.7.0...0.7.1) (2026-08-03)


### Features

* **core:** refresh pricing table to current-gen Anthropic and OpenAI models ([645cc7e](https://github.com/AtvikSecurity/domarinn/commit/645cc7e6f4a622a219ff2ee13fd5e294f1e9c0d8))

## [0.7.0](https://github.com/AtvikSecurity/domarinn/compare/0.6.2...0.7.0) (2026-08-01)


### ⚠ BREAKING CHANGES

* ChatRole gains a Tool variant and ChatMessage.content is now MessageContent rather than String. Embedders matching exhaustively on either will need updating.

### Features

* tool and content-block turns in per-case history, and validate warnings ([#57](https://github.com/AtvikSecurity/domarinn/issues/57)) ([8420ba3](https://github.com/AtvikSecurity/domarinn/commit/8420ba3419cf2dfda65709e2cf2aa057d43ee3d6))
* **web:** make sets reachable, rows clickable, and the UI usable on a phone ([#55](https://github.com/AtvikSecurity/domarinn/issues/55)) ([66d5ad8](https://github.com/AtvikSecurity/domarinn/commit/66d5ad814872bedadf21820106f9b654f1088d73))

## [0.6.2](https://github.com/AtvikSecurity/domarinn/compare/0.6.1...0.6.2) (2026-07-31)


### Features

* per-case conversation history, spliced at a prompt `history` marker ([#47](https://github.com/AtvikSecurity/domarinn/issues/47)) ([1ad6829](https://github.com/AtvikSecurity/domarinn/commit/1ad682936daa93a7f516aefde1c7315bb6ab8e14))
* **provenance:** read the branch from CI, not from a detached HEAD ([#44](https://github.com/AtvikSecurity/domarinn/issues/44)) ([68b7868](https://github.com/AtvikSecurity/domarinn/commit/68b78689586873e2d8319fd04cbac17d6d434237))


### Bug Fixes

* **ci:** stop `bash -e` aborting the eval action's binary and summary steps ([#46](https://github.com/AtvikSecurity/domarinn/issues/46)) ([046f7d7](https://github.com/AtvikSecurity/domarinn/commit/046f7d78c56ce825001b5d1511e4e296cb88ae02))

## [0.6.1](https://github.com/AtvikSecurity/domarinn/compare/0.6.0...0.6.1) (2026-07-31)


### Features

* run-set access control and a browsable Sets hierarchy ([#42](https://github.com/AtvikSecurity/domarinn/issues/42)) ([270d27e](https://github.com/AtvikSecurity/domarinn/commit/270d27e2118436511c81fdd90629db75f7bc96c5))


### Bug Fixes

* **caching:** resolve `$digest:` on a provider's cache_salt ([#38](https://github.com/AtvikSecurity/domarinn/issues/38)) ([e83e6eb](https://github.com/AtvikSecurity/domarinn/commit/e83e6eb88666da004ea0e38fd351ef696da03cbd))
* declared env keys against legacy stores, and the eval gate names the failure ([#43](https://github.com/AtvikSecurity/domarinn/issues/43)) ([bedbec3](https://github.com/AtvikSecurity/domarinn/commit/bedbec3c8ca4ce58de6286c84e44ed6d4df755b1))

## [0.6.0](https://github.com/AtvikSecurity/domarinn/compare/0.5.1...0.6.0) (2026-07-30)


### ⚠ BREAKING CHANGES

* **core:** let graded assertions see the model's tool calls ([#36](https://github.com/AtvikSecurity/domarinn/issues/36))

### Features

* **core:** let graded assertions see the model's tool calls ([#36](https://github.com/AtvikSecurity/domarinn/issues/36)) ([e9f0e30](https://github.com/AtvikSecurity/domarinn/commit/e9f0e30067189eff4f45774ab7de84791c994a15))

## [0.5.1](https://github.com/AtvikSecurity/domarinn/compare/0.5.0...0.5.1) (2026-07-30)


### Bug Fixes

* **release:** unblock release-please, and guard the message format ([#33](https://github.com/AtvikSecurity/domarinn/issues/33)) ([2933573](https://github.com/AtvikSecurity/domarinn/commit/2933573c704dfa33eb0468b35a9b186aa22e4795))

## [0.5.0](https://github.com/AtvikSecurity/domarinn/compare/0.4.0...0.5.0) (2026-07-30)


### ⚠ BREAKING CHANGES

* **caching:** provider cache keys move. A store written by 0.4.x or earlier is adopted on first lookup and re-filed, so a warm store keeps serving; `--no-cache-migration` opts out and re-pays instead.

### Features

* **caching:** one rule — key every outgoing request on its content ([#29](https://github.com/AtvikSecurity/domarinn/issues/29)) ([b914837](https://github.com/AtvikSecurity/domarinn/commit/b914837523c3b69373f6427f26a30d4877ee7985))


### Documentation

* overhaul the site — six-section IA, UI screenshots, local-LLM stack, examples 33-39 ([#31](https://github.com/AtvikSecurity/domarinn/issues/31)) ([e04c94f](https://github.com/AtvikSecurity/domarinn/commit/e04c94f2b11c9e4a928288a314f9d4cd33ef028b))

## [0.4.0](https://github.com/AtvikSecurity/domarinn/compare/0.3.1...0.4.0) (2026-07-29)


### ⚠ BREAKING CHANGES

* **caching:** key on the request, not on the machine ([#26](https://github.com/AtvikSecurity/domarinn/issues/26))
* **caching:** exec provider and exec grader cache entries are re-keyed by the switch from mtime to content digests. Existing entries are not served and will age out; no action is needed beyond expecting one cold run per suite. Network providers are unaffected.

### Features

* **caching:** key exec identity on content, correct the cache_salt docs ([#24](https://github.com/AtvikSecurity/domarinn/issues/24)) ([80479dc](https://github.com/AtvikSecurity/domarinn/commit/80479dc1e81b2b8d7214a0ec3dd739789741e235))
* **caching:** key on the request, not on the machine ([#26](https://github.com/AtvikSecurity/domarinn/issues/26)) ([c90b9f7](https://github.com/AtvikSecurity/domarinn/commit/c90b9f7880f4b2f568860e126482f4b35d4035df))
* **server:** authenticated MCP endpoint, dual-era and read-only ([#27](https://github.com/AtvikSecurity/domarinn/issues/27)) ([88739d3](https://github.com/AtvikSecurity/domarinn/commit/88739d33e36a98c20008a7f0147d64c857597926))


### Documentation

* publish a documentation site with 32 end-to-end tested examples ([#28](https://github.com/AtvikSecurity/domarinn/issues/28)) ([4d1bab7](https://github.com/AtvikSecurity/domarinn/commit/4d1bab78617a41855a3e78976eb9c814ca6e17d8))

## [0.3.1](https://github.com/AtvikSecurity/domarinn/compare/0.3.0...0.3.1) (2026-07-28)


### Features

* **caching:** clean up logic and suggestions from claude ([85ff277](https://github.com/AtvikSecurity/domarinn/commit/85ff277bbe21407809dc49a7284b66d0842b1634))

## [0.3.0](https://github.com/AtvikSecurity/domarinn/compare/0.2.0...0.3.0) (2026-07-28)


### ⚠ BREAKING CHANGES

* **core:** `asserts::evaluate_local` takes an `EvalCtx` instead of separate engine/vars/metrics arguments. Affects Rust embedders calling it directly; suite authors and exec providers are unaffected.

### Features

* **core:** real cost accounting, tool-call grading, first-class exec diagnostics, and no silently-ignored config ([#21](https://github.com/AtvikSecurity/domarinn/issues/21)) ([ee89896](https://github.com/AtvikSecurity/domarinn/commit/ee8989641b7b9d86dacf142f071d663df152d713))
* run provenance, change attribution, and a status surface ([#19](https://github.com/AtvikSecurity/domarinn/issues/19)) ([9c8c6af](https://github.com/AtvikSecurity/domarinn/commit/9c8c6af37955cd6ff6cd8227f44fac0981d3b150))

## [0.2.0](https://github.com/AtvikSecurity/domarinn/compare/0.1.3...0.2.0) (2026-07-27)


### ⚠ BREAKING CHANGES

* **release:** `releases/latest/download/domarinn-<target>` no longer resolves. GitHub has no wildcard in that path, so a versioned filename cannot have a fixed URL. Consumers resolve the tag first:

### Features

* **release:** name assets domarinn_&lt;version&gt;_linux_&lt;arch&gt; ([#17](https://github.com/AtvikSecurity/domarinn/issues/17)) ([107f586](https://github.com/AtvikSecurity/domarinn/commit/107f586babec573801a5c3e775390d7cf802137c))

## [0.1.3](https://github.com/AtvikSecurity/domarinn/compare/0.1.2...0.1.3) (2026-07-27)


### Bug Fixes

* **release:** one checksums.txt and one SBOM, instead of a per-target set ([#14](https://github.com/AtvikSecurity/domarinn/issues/14)) ([b0ed56b](https://github.com/AtvikSecurity/domarinn/commit/b0ed56b062e3f27eeba16a3623b0fd9f287d198a))

## [0.1.2](https://github.com/AtvikSecurity/domarinn/compare/0.1.1...0.1.2) (2026-07-27)


### Features

* **release:** sign binaries and publish an SBOM, and fix the checksum files ([007df97](https://github.com/AtvikSecurity/domarinn/commit/007df9739b8cbc8fabff26351da19f3822675d01))


### Bug Fixes

* **examples:** make the offline examples run for anyone who installed domarinn ([4f39910](https://github.com/AtvikSecurity/domarinn/commit/4f39910c9753ef728c85332e40055b7f56204ce1))

## [0.1.1](https://github.com/AtvikSecurity/domarinn/compare/0.1.0...0.1.1) (2026-07-27)


### Features

* **ci:** automate releases with release-please ([7d65cde](https://github.com/AtvikSecurity/domarinn/commit/7d65cde10d2a27b7579e7dd0f624ec0cb1daeeae))


### Bug Fixes

* **ci:** do not let the bot-id lookup abort the Cargo.lock sync ([0a3e7aa](https://github.com/AtvikSecurity/domarinn/commit/0a3e7aa7dadc0c40a5ee6fdebf3174083362e35d))


### Documentation

* add LICENSE and community-health files ([d87f8c9](https://github.com/AtvikSecurity/domarinn/commit/d87f8c948085d5eb0d4c28840a1d834daa4325ff))
