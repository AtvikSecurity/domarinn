// A pure, dependency-free structured diff of two JSON-ish values (config
// snapshots, here). Both sides are flattened to a `path → leaf value` map —
// objects recurse by key, arrays by index — and the two maps are compared to
// classify each leaf path as added / removed / changed. Kept pure so it is
// exhaustively unit-testable and reusable by the config-drift view.

export type JsonDiffKind = "added" | "removed" | "changed";

export interface JsonDiffEntry {
  /** Dotted path to the leaf, arrays indexed (e.g. `prompt.system`,
   *  `asserts.0.kind`). The empty string is the root for a scalar top level. */
  path: string;
  kind: JsonDiffKind;
  /** The base value; `undefined` for an `added` path. */
  before: unknown;
  /** The head value; `undefined` for a `removed` path. */
  after: unknown;
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/**
 * Flatten a value into a `path → leaf` map. A leaf is any primitive (string,
 * number, boolean, null) OR an empty object/array — so adding/removing an empty
 * container is still visible as a path. Non-empty objects/arrays recurse.
 */
export function flatten(
  value: unknown,
  prefix = "",
  out: Map<string, unknown> = new Map(),
): Map<string, unknown> {
  if (Array.isArray(value)) {
    if (value.length === 0) {
      out.set(prefix, value);
      return out;
    }
    value.forEach((el, i) =>
      flatten(el, prefix ? `${prefix}.${i}` : String(i), out),
    );
    return out;
  }
  if (isPlainObject(value)) {
    const keys = Object.keys(value);
    if (keys.length === 0) {
      out.set(prefix, value);
      return out;
    }
    for (const k of keys) flatten(value[k], prefix ? `${prefix}.${k}` : k, out);
    return out;
  }
  out.set(prefix, value);
  return out;
}

/** Structural equality for two leaf values. A stable JSON serialization catches
 *  type changes (`1` vs `"1"`), null vs value, and empty-container shape
 *  (`[]` vs `{}`). Leaves never contain `undefined`, so this is total. */
function leafEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * Diff two JSON-ish values. Returns one entry per leaf path that was added,
 * removed, or changed, sorted by path for a stable render. Identical inputs
 * yield an empty array.
 */
export function jsonDiff(before: unknown, after: unknown): JsonDiffEntry[] {
  const a = flatten(before);
  const b = flatten(after);
  const paths = new Set([...a.keys(), ...b.keys()]);

  const entries: JsonDiffEntry[] = [];
  for (const path of paths) {
    const hasA = a.has(path);
    const hasB = b.has(path);
    const va = a.get(path);
    const vb = b.get(path);
    if (hasA && !hasB) {
      entries.push({ path, kind: "removed", before: va, after: undefined });
    } else if (!hasA && hasB) {
      entries.push({ path, kind: "added", before: undefined, after: vb });
    } else if (!leafEqual(va, vb)) {
      entries.push({ path, kind: "changed", before: va, after: vb });
    }
  }

  entries.sort((x, y) => x.path.localeCompare(y.path));
  return entries;
}

/** Whether a path names a prompt-ish field (system/template/message/prompt) —
 *  the config-drift view chips these and word-diffs their string changes. */
export function isPromptPath(path: string): boolean {
  return /prompt|template|message|system/i.test(path);
}

/** Render a leaf value for the diff table: strings verbatim, everything else as
 *  compact JSON. `undefined` (an added/removed side) collapses to an em-dash. */
export function formatLeaf(value: unknown): string {
  if (value === undefined) return "—";
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}
