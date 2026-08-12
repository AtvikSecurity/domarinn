# Changelog

## [0.10.3](https://github.com/AtvikSecurity/domarinn/compare/0.10.2...0.10.3) (2026-08-12)


### Features

* Postgres backend alongside SQLite ([#92](https://github.com/AtvikSecurity/domarinn/issues/92)) ([06c6b90](https://github.com/AtvikSecurity/domarinn/commit/06c6b90004c390beaf288994375a44e2c8993c5c))
* sortable columns on every table ([#96](https://github.com/AtvikSecurity/domarinn/issues/96)) ([2665883](https://github.com/AtvikSecurity/domarinn/commit/2665883b61dfd807ed1295670a72a2cf9bb3cbb8))


### Bug Fixes

* **docker:** retry cdeps tarball downloads on transient upstream errors ([b41686f](https://github.com/AtvikSecurity/domarinn/commit/b41686f273d06c7946c66974db5482ffc6349a13))

## [0.10.2](https://github.com/AtvikSecurity/domarinn/compare/0.10.1...0.10.2) (2026-08-11)


### Features

* expected-to-fail cases — `expect_fail` with xfail/xpass statuses ([#91](https://github.com/AtvikSecurity/domarinn/issues/91)) ([3bf584b](https://github.com/AtvikSecurity/domarinn/commit/3bf584b9ee363fbb8f46e285fe189149402bbb17))
* **ui:** overhaul and adopt Atvik Design System ([a42e365](https://github.com/AtvikSecurity/domarinn/commit/a42e3656f3b2999adb0d29f9ddc9788016af2f3c))
* **web:** adopt Atvik chrome cards and outline pills ([b4f73bb](https://github.com/AtvikSecurity/domarinn/commit/b4f73bb22a196bf14735f308d1915d5023e181ab))
* **web:** bring run detail's filter groups onto the tab treatment ([b7c3383](https://github.com/AtvikSecurity/domarinn/commit/b7c33832f8c451704387e894c3a819b0069556f0))
* **web:** fill the pass-rate pill to its own percentage again ([1f52d87](https://github.com/AtvikSecurity/domarinn/commit/1f52d87ad1a89578cf167bbf6ed663375b2eb4e7))
* **web:** flatten the top bar onto the page background ([4c6c6c4](https://github.com/AtvikSecurity/domarinn/commit/4c6c6c4d8b5f48752fc797ecd4c2ce9ddfec37ea))
* **web:** give json trees and the raw view the code-block surface ([76d7554](https://github.com/AtvikSecurity/domarinn/commit/76d75540952b86f8131806a7f482f3989e13e4e6))
* **web:** give the view switchers an underline tab treatment ([0afa117](https://github.com/AtvikSecurity/domarinn/commit/0afa117ecfcb721e067af6c9345972308398e4ce))
* **web:** move the dark base canvas to [#0](https://github.com/AtvikSecurity/domarinn/issues/0)d1117 ([0c6bd4c](https://github.com/AtvikSecurity/domarinn/commit/0c6bd4cb1b6420f6ec45cb239413b89ed32cd616))
* **web:** render code through a block with a line-number gutter ([1039a7b](https://github.com/AtvikSecurity/domarinn/commit/1039a7b376f930b15a6b9852a905c2c19f26a98a))
* **web:** restyle buttons to the operator-grade recipe ([61a7a44](https://github.com/AtvikSecurity/domarinn/commit/61a7a44d2bf713d088149814938a79db6822609e))
* **web:** round the outline pills to 8px ([7a382e0](https://github.com/AtvikSecurity/domarinn/commit/7a382e0772ba1abde4107099afd5d5f1670bf3cf))
* **web:** sit the case rows on the page instead of a surface ([421d0c3](https://github.com/AtvikSecurity/domarinn/commit/421d0c351f2f2a2385068e717216aa62f3ee387e))


### Bug Fixes

* **compose:** read the public URL and cookie policy from .env ([6fcbe62](https://github.com/AtvikSecurity/domarinn/commit/6fcbe62cea9c91f21bb3f637ce835eb00c6cb75e))
* **web:** stop painting a suite with no CI run as a warning ([cdfef5c](https://github.com/AtvikSecurity/domarinn/commit/cdfef5cea29f5ae485012f737f3af12d49db73ab))
* **web:** stop the scroll hint painting a grey band on every table ([7005914](https://github.com/AtvikSecurity/domarinn/commit/70059141d3a884e3954729bf195ad3e3cf7dce03))


### Reverts

* **web:** put pills and eyebrows back in the page font ([7abe699](https://github.com/AtvikSecurity/domarinn/commit/7abe699064d3a71741e3331ca223eb4662bd9100))


### Code Refactoring

* **web:** share one detail drawer between cases and cache entries ([9dc707f](https://github.com/AtvikSecurity/domarinn/commit/9dc707f8e6de026d5b9a2f4a45b604ca7b5cb9ac))

## [0.10.1](https://github.com/AtvikSecurity/domarinn/compare/0.10.0...0.10.1) (2026-08-06)


### Features

* **cli:** fallback visibility across case detail, run output, ci-summary and diffs, plus a provider input and fallback-cases output on the eval action ([341eccd](https://github.com/AtvikSecurity/domarinn/commit/341eccd278b5633dd927994622f84e900b3eacef))
* **providers:** a fallback_only provider forms no matrix cells ([#83](https://github.com/AtvikSecurity/domarinn/issues/83)) ([341eccd](https://github.com/AtvikSecurity/domarinn/commit/341eccd278b5633dd927994622f84e900b3eacef))
* **server:** per-provider cost attribution follows the provider that answered ([341eccd](https://github.com/AtvikSecurity/domarinn/commit/341eccd278b5633dd927994622f84e900b3eacef))
* **web:** fallback attribution in the matrix and case views ([341eccd](https://github.com/AtvikSecurity/domarinn/commit/341eccd278b5633dd927994622f84e900b3eacef))


### Bug Fixes

* **cli:** the all-fallback exit gate measures graded cases, not total ([341eccd](https://github.com/AtvikSecurity/domarinn/commit/341eccd278b5633dd927994622f84e900b3eacef))

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
