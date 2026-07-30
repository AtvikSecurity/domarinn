# Changelog

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
