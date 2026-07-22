import type { CaseSearchHit, RunSearchHit, SearchResponse } from "@/api";
import { SNIPPET_CLOSE, SNIPPET_OPEN } from "@/api/snippet";
import { allRunSummaries } from "./runStats";
import { RUN_META_BY_ID } from "./runMeta";
import { fullOutput, generateCases } from "./cases";

/**
 * Fixture-side approximation of the server's FTS search: every whitespace
 * token must appear (case-insensitive substring) somewhere in the entity's
 * searchable text. Not bm25 — hits come back in fixture order — but the wire
 * shape and the snippet marker contract are the real ones, which is what the
 * UI and its tests exercise.
 */
export function searchFixtures(q: string, limit: number): SearchResponse {
  const tokens = q
    .toLowerCase()
    .split(/\s+/)
    .filter((t) => t.length > 0);
  if (tokens.length === 0) return { runs: [], cases: [] };

  const runs: RunSearchHit[] = [];
  for (const run of allRunSummaries()) {
    if (runs.length >= limit) break;
    const haystack = [
      run.project ?? "",
      run.suite ?? "",
      run.git_branch ?? "",
      run.git_commit ?? "",
      run.tags.join(" "),
    ].join(" ");
    if (matches(haystack, tokens)) {
      runs.push({
        id: run.id,
        project: run.project,
        suite: run.suite,
        created_at: run.created_at,
        snippet: snippetFor(haystack, tokens),
      });
    }
  }

  const cases: CaseSearchHit[] = [];
  for (const meta of RUN_META_BY_ID.values()) {
    if (cases.length >= limit) break;
    for (const row of generateCases(meta.id)) {
      if (cases.length >= limit) break;
      const output = fullOutput(meta, row.seed, row.status);
      const haystack = [row.name, output, row.tags.join(" ")].join(" ");
      if (matches(haystack, tokens)) {
        cases.push({
          run_id: meta.id,
          case_key: row.case_key,
          name: row.name,
          status: row.status,
          project: meta.suiteDef.project,
          suite: meta.suiteDef.suite,
          snippet: snippetFor(haystack, tokens),
        });
      }
    }
  }

  return { runs, cases };
}

function matches(haystack: string, tokens: string[]): boolean {
  const lower = haystack.toLowerCase();
  return tokens.every((t) => lower.includes(t));
}

/** A short excerpt around the first token match, with the match marked the
 *  way the server's `snippet()` marks it. */
function snippetFor(haystack: string, tokens: string[]): string {
  const first = tokens[0] ?? "";
  const at = haystack.toLowerCase().indexOf(first);
  if (at < 0) return haystack.slice(0, 80);
  const start = Math.max(0, at - 30);
  const end = Math.min(haystack.length, at + first.length + 50);
  const pre = (start > 0 ? "…" : "") + haystack.slice(start, at);
  const match = haystack.slice(at, at + first.length);
  const post = haystack.slice(at + first.length, end) + (end < haystack.length ? "…" : "");
  return `${pre}${SNIPPET_OPEN}${match}${SNIPPET_CLOSE}${post}`;
}
