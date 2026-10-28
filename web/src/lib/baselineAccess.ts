import type { GrantLevel } from "@/api";
import type { AuthView } from "./authz";

/** The two facts about a set that decide who may pin its baseline. */
export interface BaselineSetFacts {
  restricted: boolean;
  my_level: GrantLevel | null;
}

/**
 * Whether this caller may pin or unpin a suite's baseline.
 *
 * Mirrors `put_baseline`/`delete_baseline` in the server's `baselines.rs`:
 * write scope, and on a restricted set an `upload` grant or better (an admin
 * passes the set gate without one). `set` is the suite's set detail;
 * `undefined` means it is still loading, which hides the control rather than
 * flashing it at someone about to lose it, and `null` means it could not be
 * loaded, which falls back to the scope check and lets the server decide.
 */
export function canPinBaseline(
  view: Pick<AuthView, "canWrite" | "canAdmin">,
  set: BaselineSetFacts | null | undefined,
): boolean {
  if (!view.canWrite) return false;
  if (view.canAdmin || set === null) return true;
  if (set === undefined) return false;
  if (!set.restricted) return true;
  return set.my_level === "upload" || set.my_level === "manage";
}
