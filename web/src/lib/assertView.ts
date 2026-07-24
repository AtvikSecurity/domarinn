import type { AssertName } from "@/api";

/**
 * Presentation logic for one assertion row.
 *
 * An assertion row stacks up to three blocks of text that look alike and come
 * from entirely different places: the criteria the suite author wrote, the
 * reason the evaluation produced, and an optional details payload. Rendered
 * unlabelled they read as one undifferentiated wall — the reader cannot tell
 * which sentence is their own configuration and which is a grading model's
 * opinion. These helpers decide the labels and the typeface, and are pure so the
 * decisions can be tested.
 */

/**
 * The body of a criteria block, already resolved to how it should render.
 *
 * The distinction is not cosmetic. Reducing every single-field criterion to a
 * bare value produced screens like a `tokens` assertion whose whole criteria
 * block read `4000` — true, and meaningless, because the field name `max` was
 * the part that mattered. Only `value` is a generic "the criterion itself" key
 * across the assert kinds; every other name carries information.
 */
export type CriteriaBody =
  /** The generic `value` criterion — the field name adds nothing. */
  | { kind: "scalar"; text: string }
  /** Named scalar fields (`max 4000`, `min 10`), as a compact labelled list. */
  | { kind: "pairs"; pairs: [string, string][] }
  /** Anything structured: a list of alternatives, a JSON schema, a command. */
  | { kind: "json"; data: unknown };

/** The authored criteria, split into the pieces the row renders separately. */
export interface CriteriaView {
  /**
   * A numeric pass threshold, lifted out of the criteria so it can sit beside
   * the score it qualifies. `score 0.95` alone does not say whether that passed.
   */
  threshold: number | null;
  /** The assertion was authored with `negate: true`. */
  negated: boolean;
  /** What to render as the block body, or `null` when there is nothing to show. */
  body: CriteriaBody | null;
}

/** Keys that never belong in the criteria body: shown elsewhere on the row. */
const LIFTED_KEYS = new Set(["type", "negate", "threshold"]);

/**
 * The one field name that is purely structural — `contains: {value}`,
 * `llm-rubric: {value}`, `regex: {value}`. Printing the word "value" above the
 * substring or the rubric would be noise; printing "max" above `4000` is the
 * whole point.
 */
const GENERIC_KEY = "value";

/**
 * Kinds whose criteria are prose a human wrote, and so must render as prose.
 *
 * Everything else is matched character-for-character — a substring, a regex, a
 * template expression, a numeric budget — where whitespace and punctuation are
 * part of the assertion, so mono is not decoration but information.
 */
const PROSE_CRITERIA_KINDS: ReadonlySet<AssertName> = new Set<AssertName>([
  "llm-rubric",
  "similar",
]);

/** Whether this kind's criteria should render as prose rather than mono. */
export function hasProseCriteria(kind: AssertName): boolean {
  return PROSE_CRITERIA_KINDS.has(kind);
}

function finiteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** A JSON scalar that can be printed directly. `null` is excluded: printing the
 *  word "null" as if it were the criterion is worse than decomposing it. */
function scalarText(value: unknown): string | null {
  return typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
    ? String(value)
    : null;
}

/**
 * Narrow a stored `AssertResult.criteria` blob for display, or return `null`
 * when there is nothing beyond the kind to show (`is-json` carries no criteria).
 */
export function criteriaView(criteria: unknown): CriteriaView | null {
  if (criteria === null || criteria === undefined) return null;

  // A list criterion is structured data: decompose it. Stringifying would render
  // "[object Object]" for any list of objects.
  if (Array.isArray(criteria)) {
    return { threshold: null, negated: false, body: { kind: "json", data: criteria } };
  }

  const bare = scalarText(criteria);
  if (bare !== null) {
    return { threshold: null, negated: false, body: { kind: "scalar", text: bare } };
  }
  if (typeof criteria !== "object") return null;

  const obj = criteria as Record<string, unknown>;
  const threshold = finiteNumber(obj.threshold);
  const negated = obj.negate === true;
  const rest = Object.entries(obj).filter(([k]) => !LIFTED_KEYS.has(k));

  if (rest.length === 0) {
    // No body left, but a threshold or a negation is still worth reporting.
    return threshold === null && !negated
      ? null
      : { threshold, negated, body: null };
  }

  const only = rest.length === 1 ? rest[0] : undefined;
  if (only && only[0] === GENERIC_KEY) {
    const text = scalarText(only[1]);
    if (text !== null) return { threshold, negated, body: { kind: "scalar", text } };
  }

  // Named fields whose values all print directly (`min`/`max`, `command` parts):
  // keep the names, which is exactly what the scalar path would have thrown away.
  const pairs: [string, string][] = [];
  for (const [key, value] of rest) {
    const text = scalarText(value);
    if (text === null) {
      pairs.length = 0;
      break;
    }
    pairs.push([key, text]);
  }
  if (pairs.length > 0) return { threshold, negated, body: { kind: "pairs", pairs } };

  return {
    threshold,
    negated,
    body: { kind: "json", data: Object.fromEntries(rest) },
  };
}

/** How a row labels its reason text, and where that text came from. */
export interface VerdictSource {
  label: string;
  /** A short provenance note, only where mistaking the source would mislead. */
  hint?: string;
}

/**
 * Label the reason text by who produced it.
 *
 * The `llm-rubric` case is the one that matters: its reason is a second model's
 * argument for the score, sitting directly beneath the rubric the user wrote, in
 * the same box, at a similar size. Unlabelled, a grader's confident paragraph
 * reads as a statement of fact about the output.
 */
export function verdictSource(kind: AssertName): VerdictSource {
  switch (kind) {
    case "llm-rubric":
      return {
        label: "Grader verdict",
        hint: "written by the grading model, not measured",
      };
    case "exec":
      return { label: "Script result", hint: "reported by your exec assertion" };
    case "similar":
      return { label: "Similarity", hint: "computed from embeddings" };
    default:
      return { label: "Result" };
  }
}

/**
 * `0.95` — and `needs ≥ 0.70` when a threshold qualifies it.
 *
 * Returned as parts rather than one string so the score can stay prominent while
 * the threshold stays quiet.
 */
export function formatScore(score: number): string {
  return score.toFixed(2);
}

/** `needs ≥ 0.70`, or null when the assertion has no numeric threshold. */
export function formatThreshold(threshold: number | null): string | null {
  return threshold === null ? null : `needs ≥ ${threshold.toFixed(2)}`;
}
