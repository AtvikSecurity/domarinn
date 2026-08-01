import type { ProjectSetView } from "@/api";

/**
 * Finding a set by name.
 *
 * The server's `/search` indexes runs and cases — the bodies of things — and
 * has no notion of a project as a search target. So the one query people
 * actually arrive with ("take me to checkout-agent") was the one the search
 * box could not answer. `GET /sets` already returns every project the caller
 * may see, whole and unpaginated, so the match is done here rather than asked
 * for.
 *
 * Projects only, deliberately: `/sets` carries no suite names, and fetching
 * them would mean one request per project. A suite is one click further on.
 */

/**
 * Fold the separators apart so a typed phrase matches a kebab-case name.
 * Project names are written `checkout-agent`; people search "checkout agent".
 */
function normalize(value: string): string {
  return value.toLowerCase().replace(/[-_\s]+/g, " ").trim();
}

/** Lower is better. `null` means "not a match at all". */
function rank(haystack: string, needle: string): number | null {
  if (haystack === needle) return 0;
  if (haystack.startsWith(needle)) return 1;
  // A match that begins a word beats one buried mid-token: searching "agent"
  // should put `checkout-agent` above `reagent-scores`.
  if (haystack.includes(` ${needle}`)) return 2;
  if (haystack.includes(needle)) return 3;
  return null;
}

/**
 * The projects worth offering for `query`, best first.
 *
 * An empty query returns nothing. That guard lives here rather than at the
 * call sites because there are two of them, and the failure mode — the full
 * project list appearing under a blank search box — is silent.
 */
export function rankProjects(
  projects: ProjectSetView[],
  query: string,
  limit: number,
): ProjectSetView[] {
  const needle = normalize(query);
  if (!needle) return [];

  const scored: { project: ProjectSetView; score: number }[] = [];
  for (const project of projects) {
    const score = rank(normalize(project.project), needle);
    if (score !== null) scored.push({ project, score });
  }

  scored.sort(
    (a, b) =>
      a.score - b.score ||
      // Between equally-good name matches, the busier project is the more
      // likely destination.
      b.project.run_count - a.project.run_count ||
      a.project.project.localeCompare(b.project.project),
  );

  return scored.slice(0, limit).map((s) => s.project);
}
