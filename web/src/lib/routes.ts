/**
 * Client route builders.
 *
 * Every identifier in a domarinn URL is a free-text name, not a slug: projects
 * and suites are whatever a suite YAML declared, run ids are content hashes,
 * case keys are structural. So `encodeURIComponent` is not defensive here — it
 * is the only thing keeping a project called `a/b` or `100%` addressable, and
 * it was already being repeated inline at every call site. Centralizing it
 * means the encoding cannot be forgotten at the next one.
 *
 * Adopted incrementally: files get migrated when they are touched for another
 * reason, not in one sweep.
 *
 * Note the one thing this cannot fix — `encodeURIComponent` leaves `.` alone,
 * so a project literally named `..` still yields `/sets/..`. That is true of
 * every hand-rolled call site too, and is the server's problem to reject.
 */

/** The set browser's page for one project. */
export function setsPath(project: string): string {
  return `/sets/${encodeURIComponent(project)}`;
}

/**
 * One set: a project/suite pair.
 *
 * The client route drops the `/suites/` segment the REST API spells out
 * (`/api/v1/sets/{project}/suites/{suite}`). The API needs it so a suite named
 * `access` cannot collide with `/sets/{project}/access`; the client has no such
 * sibling routes, so the shorter URL is safe here.
 */
export function suitePath(project: string, suite: string): string {
  return `/sets/${encodeURIComponent(project)}/${encodeURIComponent(suite)}`;
}

/** One run's detail page. */
export function runPath(id: string): string {
  return `/runs/${encodeURIComponent(id)}`;
}

/**
 * A two-run comparison. Server contract: first segment is the base (older),
 * second is the head (newer) — `Path((id, other))` maps to `{ base, head }`.
 */
export function comparePath(baseId: string, headId: string): string {
  return `/runs/${encodeURIComponent(baseId)}/compare/${encodeURIComponent(headId)}`;
}

/**
 * The runs stream, narrowed to one set.
 *
 * Built by hand rather than with `URLSearchParams`, which encodes a space as
 * `+`. That round-trips through our own parser fine, but these URLs get pasted
 * into issues and chat, and silently rewriting every shareable link is not
 * worth the tidier call.
 *
 * `cached=all` because the stream hides fully-cached passing runs as noise;
 * arriving from a status surface, the counts have to agree.
 */
export function runsFilterHref(project: string, suite: string): string {
  return `/runs?project=${encodeURIComponent(project)}&suite=${encodeURIComponent(suite)}&cached=all`;
}
