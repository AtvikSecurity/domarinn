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
# is the one we actually expect, so check that too. The stub location and the
# target href are computed with the plugin's OWN functions (plus mkdocs' File
# for the target URL) — never re-modelled here — so index.md/README.md sides,
# #fragment and external targets, and the plugin's page-relative hrefs all
# verify exactly as written. What the plugin does NOT guard, this adds: an
# old side whose output path collides with a page that still builds would be
# silently overwritten by the stub in on_post_build, so that is an error.
# `OUT=` is a plain shell variable, so the quoted heredoc — which cannot expand
# it — only sees it if it is placed in this command's environment.
OUT="$OUT" uv run python - <<'PYEOF'
import os, pathlib, sys, yaml

from mkdocs.structure.files import File
from mkdocs_redirects.plugin import get_html_path, get_relative_html_path


class MkDocsConfigLoader(yaml.SafeLoader):
    """SafeLoader that tolerates every non-core tag mkdocs.yml may carry.

    toc.slugify and superfences' custom_fences wire !!python/... callables,
    and mkdocs accepts !ENV anywhere; we only read `plugins`, `docs_dir` and
    `use_directory_urls` here, so swallow the tags rather than reject a
    config mkdocs itself considers valid.
    """


for tag in ("tag:yaml.org,2002:python/", "!"):
    MkDocsConfigLoader.add_multi_constructor(tag, lambda loader, suffix, node: None)

config = yaml.load(pathlib.Path("mkdocs.yml").read_text(), Loader=MkDocsConfigLoader)

# `plugins:` is equally valid as a list (of names or one-key mappings) or as
# one mapping; normalize to name -> options so neither shape slips through
# with its redirects unchecked.
plugins_cfg = config.get("plugins") or []
plugins = {}
if isinstance(plugins_cfg, dict):
    plugins = {str(name): options or {} for name, options in plugins_cfg.items()}
else:
    for entry in plugins_cfg:
        if isinstance(entry, dict):
            plugins.update({str(k): v or {} for k, v in entry.items()})
        else:
            plugins[str(entry)] = {}
redirect_maps = plugins.get("redirects", {}).get("redirect_maps") or {}

use_directory_urls = config.get("use_directory_urls", True)
site_dir = pathlib.Path(os.environ["OUT"])
docs_dir = pathlib.Path(config.get("docs_dir", "docs"))

# Output paths of every page really built from docs/. The plugin writes each
# stub unconditionally in on_post_build, after the build proper.
built = {
    get_html_path(str(page.relative_to(docs_dir)), use_directory_urls)
    for page in docs_dir.rglob("*.md")
}

errors = []
verified = 0
for old, new in redirect_maps.items():
    if not old.endswith(".md"):
        errors.append(f"{old} -> {new}: the old side must be a docs/-relative .md path")
        continue

    stub_rel = get_html_path(old, use_directory_urls)
    if stub_rel in built:
        errors.append(
            f"{old} -> {new}: {stub_rel} is also the output of a page that "
            f"still exists under {docs_dir}/ — the plugin would overwrite "
            f"that page with the redirect stub"
        )
        continue

    if new.lower().startswith(("http://", "https://")):
        expected = new  # external targets are written into the stub verbatim
    else:
        new_path, sep, fragment = new.partition("#")
        if not new_path.endswith(".md"):
            errors.append(
                f"{old} -> {new}: the new side must be a docs/-relative .md "
                f"path (optionally with a #fragment) or an http(s) URL"
            )
            continue
        # Exactly what on_post_build hands write_html: the built page's final
        # URL, fragment reattached, made relative to the stub's directory.
        new_url = File(new_path, "", "", use_directory_urls).url + sep + fragment
        expected = get_relative_html_path(old, new_url, use_directory_urls)

    stub = site_dir / stub_rel
    if not stub.is_file():
        errors.append(f"{old}: expected redirect stub {stub} was not written")
    elif f'"{expected}"' not in stub.read_text():
        errors.append(f"{old}: {stub} does not point at {expected}")
    else:
        verified += 1

if errors:
    print("redirect check FAILED:", *errors, sep="\n  ", file=sys.stderr)
    sys.exit(1)
print(f"    {verified} redirects verified")
PYEOF

echo "==> style gates (docs/ link + fence conventions)"
# Blocking. Both conventions hold across docs/ as of the documentation
# restructure, so a fresh violation is a slip in review rather than a backlog
# item — and each is invisible in the rendered page, which is exactly the kind
# of drift a human reviewer stops noticing.
#
# awk, not `head`: under pipefail, `printf … | head -5` dies of SIGPIPE the
# moment the match list outgrows the pipe buffer — which would replace the
# gate's own diagnosis with a broken-pipe failure exactly when the violations
# are most numerous. awk reads its whole input, so the pipeline always exits 0.
#
# Every violation is printed, not a sample: the caller has to fix all of them,
# and a truncated list makes that two build cycles instead of one.
style_gate() {
  local pattern="$1" label="$2" fix="$3" matches
  matches="$(grep -rn "$pattern" docs || true)"
  if [ -z "$matches" ]; then
    echo "    0 ${label} in docs/"
    return 0
  fi
  {
    echo "style gate FAILED: $(printf '%s\n' "$matches" | wc -l) ${label} in docs/"
    echo "  fix: ${fix}"
    printf '%s\n' "$matches" | awk '{ print "    " $0 }'
  } >&2
  return 1
}

# `|| violations=…` keeps each call in a condition context, so `set -e` does not
# abort on the first failing gate — one build reports both.
style_violations=0
style_gate '](\./' "'./'-relative link(s)" \
  'write [text](sibling.md), not [text](./sibling.md)' || style_violations=$((style_violations + 1))
style_gate '^```bash' "'\`\`\`bash' fenced block(s)" \
  "use \`\`\`sh for a script, or \`\`\`console for a session with prompts" || style_violations=$((style_violations + 1))
if [ "$style_violations" -gt 0 ]; then
  echo "style gates FAILED: ${style_violations} convention(s) violated under docs/" >&2
  exit 1
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
