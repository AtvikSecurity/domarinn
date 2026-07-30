# Install domarinn

domarinn ships as one static binary — a CLI, an eval engine, and a self-hostable results server. Four ways to get it, in the order most people reach for them.

**Prebuilt binary** (fastest; no toolchain needed). Each release publishes a fully static musl binary for `x86_64` and `aarch64`, so it runs on any Linux distro with no shared-library requirements:

```sh
arch=amd64            # or arm64
rel=https://github.com/AtvikSecurity/domarinn/releases

# Asset names carry the version, so there is no fixed "latest" URL to curl.
# GitHub redirects /releases/latest to /releases/tag/<tag>, which is enough to
# learn the tag without an API token or a JSON parser. Set `ver` by hand
# instead — `ver=0.2.0` — to pin.
ver=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$rel/latest" | sed 's#.*/tag/##')

bin="domarinn_${ver}_linux_${arch}"
curl -fsSLO "$rel/download/$ver/$bin"
curl -fsSLO "$rel/download/$ver/domarinn_${ver}_checksums.txt"

# --ignore-missing because the manifest covers every artifact in the release
# and you only downloaded one; without it sha256sum reports the others as
# missing and exits non-zero. It still fails loudly if nothing matched at all,
# so this cannot silently verify zero files.
sha256sum --ignore-missing --check "domarinn_${ver}_checksums.txt"

chmod +x "$bin"
sudo mv "$bin" /usr/local/bin/domarinn
domarinn --version
```

Release tags are bare semver — `0.2.0`, not `v0.2.0` — and the same string appears in every asset name, so a downloaded file always says which release it came from.

## Verifying a download

Every release artifact is signed with [cosign](https://docs.sigstore.dev/) using keyless signing — there is no public key to fetch, because the signing identity *is* the GitHub Actions workflow that built it. The `.sigstore.json` bundle carries the certificate, the signature and the transparency-log entry together:

```sh
curl -fsSLO "$rel/download/$ver/$bin.sigstore.json"

cosign verify-blob \
  --bundle "$bin.sigstore.json" \
  --certificate-identity-regexp '^https://github\.com/AtvikSecurity/domarinn/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$bin"
```

Keep the `--certificate-identity-regexp`. Without it cosign will accept a signature from *any* identity, which verifies that the file was signed by someone rather than that it was signed by this project.

The same command verifies the checksum manifest and the SBOM — substitute either filename for `$bin` in both the `--bundle` and the final argument.

Each release also publishes an SPDX SBOM — `domarinn_<version>.spdx.json`, itself signed — cataloguing the Rust and npm dependency graphs that went into the binary. There is one per release, not one per architecture: it is generated from `Cargo.lock` and `web/pnpm-lock.yaml`, which say nothing about the target triple. Note that it does not come from the shipped binary either — release builds are stripped, so scanning the binary would report almost nothing.

**With cargo.** domarinn is not published to crates.io, so `cargo install domarinn-cli` will not find it. Install from git instead:

```sh
cargo install --git https://github.com/AtvikSecurity/domarinn --locked domarinn-cli

# or pin to a published release tag (bare semver, no leading v)
cargo install --git https://github.com/AtvikSecurity/domarinn --tag <version> --locked domarinn-cli
```

This builds the CLI only. The eval engine is complete, but the binary does not embed the web UI, so `domarinn server` serves a placeholder page — use the prebuilt binary, the container, or `mise run install` if you want the UI.

Keep `--locked`: it makes the build use the exact dependency versions the release was tested with.

**From source** (needs a recent Rust toolchain):

```sh
git clone https://github.com/AtvikSecurity/domarinn
cd domarinn
cargo build --release            # binary at target/release/domarinn
# with the web UI embedded (via mise — https://mise.jdx.dev):
mise run build                   # builds web/dist, then the release binary

# or install `domarinn` straight into ~/.cargo/bin (on your PATH):
mise run install                 # full binary, web UI embedded
mise run install-cli             # eval CLI only, no web UI (fast)
mise run install-musl            # static musl single binary (needs a musl C toolchain)
```

**Docker** (the server + embedded UI):

```sh
docker run -p 8321:8321 -v domarinn-data:/data ghcr.io/atviksecurity/domarinn:rolling
```

`rolling` tracks the tip of `main`. Releases also publish semver tags — `0.1.2`, `0.1`, `0` — so pin one of those for anything you care about staying still. There is deliberately no `latest`, precisely so an unpinned tag cannot silently jump a major.

Put `target/release/domarinn` on your `PATH`, or invoke it directly.

## Next

[Your first eval](first-eval.md) — write and run a suite in a few minutes, offline and with no API key.
