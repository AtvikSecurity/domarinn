// Column accessors for sorting run-list tables client-side. Shared by the
// runs page and the suite page's run table (a deliberate copy of the same
// table); each table sorts only the columns it actually renders, so a key
// with no matching column is simply never consulted.
//
// Client-side means loaded rows only — both tables are cursor paginated and
// show a "(sorted within loaded runs)" caveat while more pages remain, the
// same honesty rule as the case grid.

import type { RunListItem } from "@/api";
import { parseTimestamp } from "@/lib/format";
import type { SortAccessor } from "@/lib/sort";

export const RUN_SORT_FIELDS: Record<string, SortAccessor<RunListItem>> = {
  // ULIDs are lexically chronological, so this doubles as "by creation".
  run: (r) => r.id,
  when: (r) => parseTimestamp(r.created_at),
  // What RunOriginCell leads with: the recorded actor, else the uploader.
  who: (r) => r.actor ?? r.uploaded_by,
  branch: (r) => r.git_branch,
  // The server's authoritative rate — not recomputed from the counts.
  pass_rate: (r) => r.pass_rate,
  cases: (r) => r.case_count,
  tokens: (r) => r.prompt_tokens + r.completion_tokens,
  cost: (r) => r.cost_usd,
  duration: (r) => r.duration_ms,
  // Tag-less runs sort last (null), not as the empty string.
  tags: (r) => (r.tags.length ? r.tags.join(",") : null),
};
