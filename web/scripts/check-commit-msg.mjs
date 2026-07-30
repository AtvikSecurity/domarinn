#!/usr/bin/env node
// Reject a commit message release-please cannot parse.
//
// # Why this exists
//
// A commit whose message does not parse is not a warning — release-please
// SKIPS it. If it is the only commit since the last release, the run reports
// "No commits for path: ., skipping" and no release pull request is created at
// all. Worse, the skip is permanent: every later run re-parses the same commit,
// fails the same way, and the work it carried never reaches a changelog.
//
// That happened on 0.5.0 -> 0.6.0. One body line read:
//
//     sha256(canonical_json({request, repeat, salts})) — the only way to …
//
// A body line that STARTS with `identifier(` is read as a conventional-commit
// header with a scope, so the second `(` before the closing `)` is a syntax
// error. The same line with any leading word ("A key is sha256(…") parses
// fine, which is what makes this so easy to write and so invisible in review.
//
// # Why the real parser and not a regex
//
// The rule above is the one failure we hit, not the whole grammar. This runs
// the exact parser release-please 17.x uses, so the check cannot drift from the
// tool it is protecting — a regex approximating a grammar would pass messages
// the real parser rejects, which is the failure mode this is meant to end.
//
// # Why this lives under web/
//
// `web/` is the repo's only node package, so this is where a node dependency
// can be installed and resolved. The script is repo tooling rather than
// frontend code; it is here for module resolution, nothing more.

import fs from "node:fs";
import { parser } from "@conventional-commits/parser";

/**
 * Messages git generates or that are not meant to be conventional.
 *
 * These reach a `commit-msg` hook like any other and would all fail the
 * grammar. Rejecting them would break `git merge`, `git revert` and the
 * autosquash workflow, none of which is what this guards.
 */
const EXEMPT = [
  /^Merge /, // git merge / GitHub merge commits
  /^Revert "/, // git revert (note: a hand-written `revert:` type still parses)
  /^(fixup|squash|amend)!/, // rebase autosquash
  /^Signed-off-by:/,
];

/** Strip the comment lines git appends to the message buffer. */
function stripComments(raw) {
  return raw
    .split("\n")
    .filter((line) => !line.startsWith("#"))
    .join("\n")
    .trim();
}

/**
 * The specific hint for the failure that motivated this script.
 *
 * A generic parser error points at a line and column and leaves the author to
 * work out what a "valid token" would have been. Naming the shape turns that
 * into a one-word fix.
 */
function hintFor(message) {
  const lines = message.split("\n");
  // Skip the header; a scope there is legitimate.
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i];
    if (/^[A-Za-z0-9_.\-/]+\(/.test(line) && (line.match(/\(/g) ?? []).length > 1) {
      return (
        `line ${i + 1} starts with \`${line.split("(")[0]}(\` and contains another ` +
        `\`(\` before its closing \`)\`.\n\n` +
        `  ${line.trim()}\n\n` +
        `A body line beginning with \`identifier(\` is parsed as a conventional-commit ` +
        `header, so the\nsecond \`(\` is a syntax error. Put any word in front of it ` +
        `— "A key is sha256(…" parses fine —\nor reword to avoid the leading call.`
      );
    }
  }
  return null;
}

function check(message) {
  const trimmed = stripComments(message);
  if (!trimmed) return { ok: true, skipped: "empty" };
  if (EXEMPT.some((re) => re.test(trimmed))) return { ok: true, skipped: "exempt" };
  try {
    parser(trimmed);
    return { ok: true };
  } catch (error) {
    return { ok: false, error: error.message, hint: hintFor(trimmed) };
  }
}

/** Guard on the guard: the message that actually broke a release must fail. */
function selfTest() {
  const cases = [
    ["the 0.5.0 breakage", "feat(cache): x\n\nsha256(canonical_json({a, b})) — so.\n", false],
    ["same line, led by a word", "feat(cache): x\n\nA key is sha256(canonical_json({a, b})) — so.\n", true],
    ["ordinary parens", "fix(server): x\n\nSomething (worth noting) here.\n", true],
    ["plain conventional", "feat: add a thing\n", true],
    ["merge commit", "Merge branch 'main' into feat/x\n", true],
    ["revert commit", 'Revert "feat: a thing"\n\nThis reverts commit abc.\n', true],
    ["autosquash", "fixup! feat(cache): x\n", true],
  ];
  let failed = 0;
  for (const [name, message, shouldPass] of cases) {
    const got = check(message).ok;
    const good = got === shouldPass;
    if (!good) failed++;
    console.log(`${good ? "ok  " : "FAIL"}  ${name} (expected ${shouldPass ? "pass" : "reject"})`);
  }
  if (failed) {
    console.error(`\n${failed} self-test case(s) wrong — this checker cannot be trusted.`);
    process.exit(1);
  }
  console.log("\nself-test passed");
}

// `pnpm run <script> -- --self-test` forwards the bare `--` to the script, so
// it arrives as argv[2] and would be read as a filename. Dropping it here means
// the script behaves the same however it is invoked — directly, through
// `pnpm run`, or through `pnpm exec`.
const [arg] = process.argv.slice(2).filter((a) => a !== "--");
if (arg === "--self-test") {
  selfTest();
} else if (!arg) {
  console.error("usage: check-commit-msg.mjs <message-file> | --self-test");
  process.exit(2);
} else {
  const result = check(fs.readFileSync(arg, "utf8"));
  if (!result.ok) {
    console.error("\nThis commit message is not parseable by release-please.\n");
    console.error(`  parser: ${result.error}`);
    if (result.hint) console.error(`\n  ${result.hint}`);
    console.error(
      "\nAn unparseable commit is silently SKIPPED by release-please, so the work it\n" +
        "carries never reaches a changelog — and if it is the only commit since the last\n" +
        "release, no release pull request is created at all.\n",
    );
    process.exit(1);
  }
}
