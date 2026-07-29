#!/usr/bin/env bash
# build-docs.sh — assemble the full domarinn documentation site.
#
# Produces one directory ready for GitHub Pages:
#   site/            ← MkDocs Material user docs (site root)
#   site/rustdoc/    ← `cargo doc` for the publishable domarinn-protocol crate
#
# `mkdocs build --strict` fails on a broken intra-site link or a missing nav
# file (validation config in mkdocs.yml), so this script doubles as our doc lint.
#
# Run via `mise run docs` so uv — and therefore the uv.lock-pinned MkDocs +
# Material + pymdown-extensions — resolves to the versions pinned in the repo.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

OUT="site"
# The crate the rustdoc redirect lands on. domarinn-protocol is the one third
# parties actually depend on — it is what you write a provider, an assertion or
# a test generator against — so it is the natural companion to docs/protocol.md.
LANDING_CRATE="domarinn_protocol"
# Custom domain the site is served from. Written into the artifact as a CNAME so
# the domain survives every deploy (must match Settings -> Pages custom domain).
SITE_DOMAIN="docs.domarinn.com"

echo "==> cargo doc (workspace, no deps)"
# `target/doc` is shared and additive: rustdoc never removes a crate a previous
# invocation wrote there, so `cp -r` below would otherwise publish whatever some
# earlier `cargo doc` happened to leave behind. Clearing it first is what makes
# the artifact a function of the source rather than of the build history.
rm -rf target/doc
cargo doc --no-deps --workspace --locked

echo "==> checking snippet SECTION references (--strict only guards file paths)"
# pymdownx.snippets' check_paths fails the build on a missing FILE, but a
# `--8<-- "file.yaml:section"` whose section marker was renamed or removed
# renders as a silently EMPTY code block — exactly the docs/example drift the
# snippet convention exists to prevent. Verify every section reference resolves.
uv run python - <<'PYEOF'
import pathlib, re, sys

errors = []
for md in pathlib.Path("docs").rglob("*.md"):
    for path_str, section in re.findall(r'--8<--\s+"([^":]+):([^"]+)"', md.read_text()):
        path = pathlib.Path(path_str)
        if re.fullmatch(r"\d*(:\d*)?", section):
            continue  # a LINE-RANGE include (file:5:8), not a named section
        if not path.is_file():
            continue  # missing files are check_paths' job; don't double-report
        if f"--8<-- [start:{section}]" not in path.read_text():
            errors.append(f"{md}: section '{section}' not found in {path} "
                          f"(expected a '# --8<-- [start:{section}]' marker)")
if errors:
    print("snippet section check FAILED:", *errors, sep="\n  ", file=sys.stderr)
    sys.exit(1)
PYEOF

echo "==> checking that inline YAML snippets parse"
# A ```yaml block that is TRANSCLUDED (--8<--) is the real file, and
# crates/domarinn-cli/tests/examples.rs runs it. A block typed inline into a
# page is checked by nothing at all — and a mis-indented `value:` renders
# perfectly while being unusable by anyone who copies it. Parse them.
uv run python - <<'PYEOF'
import pathlib, re, sys, yaml


class DomarinnLoader(yaml.SafeLoader):
    """SafeLoader that tolerates domarinn's own tags.

    `!raw` and `!file` are load-bearing suite syntax, so a plain SafeLoader
    rejects every page that documents them — which is most of the good ones.
    """


DomarinnLoader.add_multi_constructor("!", lambda loader, suffix, node: None)

errors = []
checked = 0
for md in sorted(pathlib.Path("docs").rglob("*.md")):
    for block in re.findall(r"```yaml\n(.*?)```", md.read_text(), re.S):
        if "--8<--" in block:
            continue  # the real file; the Rust harness runs it
        checked += 1
        try:
            list(yaml.load_all(block, Loader=DomarinnLoader))
        except yaml.YAMLError as exc:
            first = str(exc).splitlines()[0]
            errors.append(f"{md}: {first}")

if errors:
    print("inline YAML check FAILED:", *errors, sep="\n  ", file=sys.stderr)
    sys.exit(1)
print(f"    {checked} inline YAML snippets parse")
PYEOF

echo "==> mkdocs build (--strict: a broken link or missing nav file fails here)"
# uv run resolves MkDocs + plugins from the committed uv.lock into a managed venv.
uv run mkdocs build --strict --site-dir "$OUT"

echo "==> checking redirect stubs against mkdocs.yml's redirect_maps"
# mkdocs-redirects (wired in mkdocs.yml's plugins:) already warns — and
# --strict above already fails the build on — a redirect_maps entry whose
# target page doesn't exist. It says nothing about whether the stub it wrote
# is the one we actually expect, so check that too: parse redirect_maps
# straight out of mkdocs.yml (a small heredoc, not hand-rolled YAML in bash)
# and, for each entry, assert the old page's stub exists under site_dir and
# points at the new page.
OUT="$OUT" uv run python - <<'PYEOF'
import os, pathlib, sys, yaml


class MkDocsConfigLoader(yaml.SafeLoader):
    """SafeLoader that tolerates mkdocs.yml's !!python/... tags.

    toc.slugify and superfences' custom_fences each wire a Python callable
    that way; we only read `plugins` here, so swallow those tags rather than
    choke on them.
    """


MkDocsConfigLoader.add_multi_constructor(
    "tag:yaml.org,2002:python/", lambda loader, suffix, node: None
)

config = yaml.load(pathlib.Path("mkdocs.yml").read_text(), Loader=MkDocsConfigLoader)

redirect_maps = {}
for entry in config.get("plugins", []):
    if isinstance(entry, dict) and "redirects" in entry:
        redirect_maps = (entry["redirects"] or {}).get("redirect_maps") or {}
        break

site_dir = pathlib.Path(os.environ["OUT"])

errors = []
verified = 0
for old, new in redirect_maps.items():
    # use_directory_urls is on, so 'getting-started.md' -> a stub at
    # 'site/getting-started/index.html' that points at the new page's own
    # directory-style URL. An index.md-style path (either side) instead maps
    # to its *section root* — a different shape this checker doesn't attempt
    # to follow. The map won't contain one for now; error loudly rather than
    # silently checking the wrong file if that ever changes.
    if pathlib.Path(old).name == "index.md" or pathlib.Path(new).name == "index.md":
        errors.append(f"{old} -> {new}: index.md-style paths are unsupported here")
        continue
    if not old.endswith(".md") or not new.endswith(".md"):
        errors.append(f"{old} -> {new}: both sides must be docs/-relative .md paths")
        continue

    stub = site_dir / old[: -len(".md")] / "index.html"
    new_url = new[: -len(".md")] + "/"
    if not stub.is_file():
        errors.append(f"{old}: expected redirect stub {stub} was not written")
    elif new_url not in stub.read_text():
        errors.append(f"{old}: {stub} does not point at {new_url}")
    else:
        verified += 1

if errors:
    print("redirect check FAILED:", *errors, sep="\n  ", file=sys.stderr)
    sys.exit(1)
print(f"    {verified} redirects verified")
PYEOF

echo "==> style gates (docs/ link + fence conventions; report-only for now)"
# TODO(docs-overhaul): flip both of these to blocking once the docs
# restructure normalizes the patterns they report across docs/.
RELATIVE_LINKS="$(grep -rn '](\./' docs || true)"
if [ -z "$RELATIVE_LINKS" ]; then
  echo "    0 './'-relative links in docs/"
else
  echo "    $(printf '%s\n' "$RELATIVE_LINKS" | wc -l) './'-relative link(s) in docs/ (sample, up to 5):"
  printf '%s\n' "$RELATIVE_LINKS" | head -5 | sed 's/^/      /'
fi

# TODO(docs-overhaul): flip to blocking once docs/ fences are normalized to
# sh/console/bare (the repo convention).
BASH_FENCES="$(grep -rn '^```bash' docs || true)"
if [ -z "$BASH_FENCES" ]; then
  echo "    0 '\`\`\`bash' fenced blocks in docs/"
else
  echo "    $(printf '%s\n' "$BASH_FENCES" | wc -l) '\`\`\`bash' fenced block(s) in docs/ (sample, up to 5):"
  printf '%s\n' "$BASH_FENCES" | head -5 | sed 's/^/      /'
fi

echo "==> nesting rustdoc under ${OUT}/rustdoc"
rm -rf "${OUT}/rustdoc"
cp -r target/doc "${OUT}/rustdoc"

# `cargo doc` emits no root index.html, so add a redirect into the entry crate.
cat > "${OUT}/rustdoc/index.html" <<EOF
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta http-equiv="refresh" content="0; url=${LANDING_CRATE}/index.html">
    <link rel="canonical" href="${LANDING_CRATE}/index.html">
    <title>domarinn protocol API reference</title>
  </head>
  <body>
    <p>Redirecting to <a href="${LANDING_CRATE}/index.html">the domarinn protocol API reference</a>…</p>
  </body>
</html>
EOF

# GitHub Pages deploys via Actions do not run Jekyll, but rustdoc emits
# _-prefixed paths; .nojekyll keeps it explicit and future-proof.
touch "${OUT}/.nojekyll"

# Pin the custom domain in the published artifact. Without this file the domain
# is cleared on the next deploy and the site falls back to the github.io URL.
echo "${SITE_DOMAIN}" > "${OUT}/CNAME"

echo "==> docs site assembled at ${OUT}/ (mkdocs + rustdoc) for https://${SITE_DOMAIN}/"
