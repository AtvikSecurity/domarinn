// Small pure formatters used across pages.

const numberFmt = new Intl.NumberFormat("en-US");

/** Short display form of a run id. Real run ids are content-hash idempotency
 *  keys (see the generated `RunId` doc) — an opaque hex hash collapses to a
 *  git-style 12-char prefix, while the demo's readable slug ids (which contain
 *  non-hex letters) are already scannable and pass through unchanged. */
export function shortRunId(id: string): string {
  if (id.length > 16 && /^[0-9a-f]+$/i.test(id)) return id.slice(0, 12);
  return id;
}

export function formatInt(n: number | undefined | null): string {
  if (n === undefined || n === null) return "-";
  return numberFmt.format(Math.round(n));
}

/** Compact token counts: 1234 -> "1.2k", 1_200_000 -> "1.2M". */
export function formatTokens(n: number | undefined | null): string {
  if (n === undefined || n === null) return "-";
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

export function formatCost(usd: number | undefined | null): string {
  if (usd === undefined || usd === null) return "-";
  if (usd === 0) return "$0.00";
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
}

export function formatLatency(ms: number | undefined | null): string {
  if (ms === undefined || ms === null) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

export function formatDuration(ms: number | undefined | null): string {
  if (ms === undefined || ms === null) return "-";
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rem = Math.round(s % 60);
  if (m < 60) return `${m}m ${rem}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

export function formatBytes(bytes: number | undefined | null): string {
  if (bytes === undefined || bytes === null) return "-";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let n = bytes;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

/** pass_count / (pass+fail+error). Returns 0..1, or null if no evaluated cases. */
export function passRate(
  pass: number,
  fail: number,
  error: number,
): number | null {
  const denom = pass + fail + error;
  if (denom === 0) return null;
  return pass / denom;
}

export function formatPercent(ratio: number | null, digits = 1): string {
  if (ratio === null) return "-";
  return `${(ratio * 100).toFixed(digits)}%`;
}

// Dense form for table rows and tooltips-within-tooltips: no year, no zone.
const dateFmt = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

// Unambiguous form, for anywhere a reader needs to know *exactly* when. CI runs
// in UTC and the browser renders local, so the zone is not optional; and a run
// from last March rendered "Mar 3, 02:15" without a year.
const dateAbsoluteFmt = new Intl.DateTimeFormat("en-US", {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  timeZoneName: "short",
});

/**
 * RFC3339 -> epoch millis. The server emits every timestamp field
 * (`created_at`, `uploaded_at`, `last_used_at`, `last_run_at`,
 * `oldest_entry_at`, ...) as an RFC3339 string; this is the one place that
 * converts one to a number for date math (sorting, deltas) or formatting.
 * Returns `NaN` for an unparseable string, same as the underlying
 * `Date.parse` — never throws.
 */
export function parseTimestamp(iso: string): number {
  return Date.parse(iso);
}

export function formatDate(iso: string | undefined | null): string {
  if (!iso) return "-";
  const ms = parseTimestamp(iso);
  if (Number.isNaN(ms)) return "-";
  return dateFmt.format(new Date(ms));
}

/** "3h ago", "2d ago" — coarse relative time for list rows. */
export function formatRelative(
  iso: string | undefined | null,
  now: number = Date.now(),
): string {
  if (!iso) return "-";
  const ms = parseTimestamp(iso);
  if (Number.isNaN(ms)) return "-";
  const diff = now - ms;
  const abs = Math.abs(diff);
  const min = 60_000;
  const hour = 60 * min;
  const day = 24 * hour;
  const suffix = diff >= 0 ? "ago" : "from now";
  if (abs < min) return "just now";
  if (abs < hour) return `${Math.round(abs / min)}m ${suffix}`;
  if (abs < day) return `${Math.round(abs / hour)}h ${suffix}`;
  if (abs < 30 * day) return `${Math.round(abs / day)}d ${suffix}`;
  // Past the relative window the reader needs the year and the zone — this is
  // the fallback that used to render a bare, year-less "Mar 3, 02:15".
  return formatDateAbsolute(iso);
}

/** Full date with year and time zone. Use in tooltips and `title`s. */
export function formatDateAbsolute(iso: string | undefined | null): string {
  if (!iso) return "-";
  const ms = parseTimestamp(iso);
  if (Number.isNaN(ms)) return "-";
  return dateAbsoluteFmt.format(new Date(ms));
}

/**
 * The grid's row-count summary.
 *
 * There is no server-side filtered total — `CaseListResponse` carries only the
 * page and a cursor — so the honest statement depends on whether more pages
 * exist. With a filter applied and no next page, the loaded count IS the total,
 * and the old "Showing 48 of 144+ cases" was simply wrong.
 */
export function formatCaseCount(
  loaded: number,
  total: number | null | undefined,
  hasNextPage: boolean,
): string {
  const noun = loaded === 1 ? "case" : "cases";
  if (!hasNextPage) return `Showing ${loaded} ${noun}`;
  if (total != null && total > loaded) {
    return `Showing first ${loaded} of ${total}+ ${noun}`;
  }
  return `Showing first ${loaded} ${noun}`;
}
