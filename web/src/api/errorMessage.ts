import { ApiError } from "./client";

/**
 * The sentence to show a user when a request failed.
 *
 * The server answers a refused write with `{ "error": "<human sentence>" }` —
 * "username already exists", "cannot remove the last admin", "password must be
 * at least 8 characters". That sentence is the remediation, so it wins over
 * any generic copy. Pages used to match machine codes (`last_admin`,
 * `username_taken`) that only the mocks ever sent; against a real server the
 * match never fired and every refusal read as "Could not create the user."
 *
 * `fallback` is for everything else: a network failure, a 5xx with no body, or
 * a body that is not the `{ error }` shape.
 */
export function apiErrorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) {
    const body = err.body;
    if (body && typeof body === "object" && "error" in body) {
      const msg = body.error;
      if (typeof msg === "string" && msg.trim() !== "") return msg;
    }
    if (err.status === 403) return "You don't have permission to do that.";
  }
  return fallback;
}
