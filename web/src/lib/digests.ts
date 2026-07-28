import type { ComponentDrift } from "@/api";

/**
 * Which parts of a suite moved between two runs.
 *
 * The whole-suite `config_digest` can only say "config changed", which is the
 * same answer for a prompt rewrite and a typo in a description. These name the
 * component, so a reader can tell "you changed the prompt, of course the output
 * differs" from "nothing changed and it regressed anyway".
 */

/** Display order — most likely to explain a change first. */
export const COMPONENT_ORDER = [
  "prompts",
  "providers",
  "tests",
  "asserts",
  "grader",
] as const;

/** Short labels; `providers` reads as "model" to anyone looking at results. */
const LABELS: Record<string, string> = {
  prompts: "prompts",
  providers: "model",
  tests: "tests",
  asserts: "asserts",
  grader: "grader",
};

export function componentLabel(component: string): string {
  return LABELS[component] ?? component;
}

/**
 * The components that definitely changed.
 *
 * `changed: null` means unknown — one side predates component digests — and is
 * deliberately NOT reported as changed. Claiming a change we cannot see would
 * invent a finding; the caller shows nothing instead.
 */
export function changedComponents(drift: ComponentDrift[]): string[] {
  const changed = drift.filter((d) => d.changed === true).map((d) => d.component);
  return changed.sort(
    (a, b) => orderOf(a) - orderOf(b) || a.localeCompare(b),
  );
}

/** True when at least one component's state is unknowable. */
export function hasUnknownComponents(drift: ComponentDrift[]): boolean {
  return drift.some((d) => d.changed === null);
}

function orderOf(component: string): number {
  const i = (COMPONENT_ORDER as readonly string[]).indexOf(component);
  return i === -1 ? COMPONENT_ORDER.length : i;
}
