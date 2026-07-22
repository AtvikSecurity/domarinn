// Pure, framework-free helpers for the SSO login buttons. Kept out of the
// component so redirect-safety and error-copy logic are unit-testable.

/**
 * A same-origin path safe to hand the server as a post-login redirect target.
 * It must start with a single "/" that is not followed by another "/" or a
 * "\" — browsers normalize a leading "\" to "/", so "/\evil.com" (or "//evil")
 * resolves to the absolute URL "https://evil.com" and is an open redirect.
 * Anything not clearly a same-origin path collapses to "/".
 */
export function safeRedirectPath(raw: string | null | undefined): string {
  if (raw && /^\/(?![/\\])/.test(raw)) return raw;
  return "/";
}

/**
 * Append `return_to=<encoded path>` to a provider's `login_url`, choosing "?"
 * or "&" depending on whether the URL already has a query string.
 */
export function buildSsoStartUrl(loginUrl: string, redirectPath: string): string {
  const sep = loginUrl.includes("?") ? "&" : "?";
  return `${loginUrl}${sep}return_to=${encodeURIComponent(redirectPath)}`;
}

/** Human copy for a `?sso_error=<code>` on the login page; null when absent. */
export function ssoErrorMessage(code: string | null | undefined): string | null {
  if (!code) return null;
  switch (code) {
    case "access_denied":
      return "Sign-in was cancelled or denied by the identity provider.";
    case "invalid_state":
      return "The sign-in session expired or did not match. Please try again.";
    case "expired":
      return "The sign-in request timed out. Please try again.";
    case "email_not_allowed":
      return "Your account's email domain is not permitted on this server.";
    case "replayed":
      return "This sign-in response was already used. Please start again.";
    case "account_disabled":
      return "Your account is disabled. Contact an administrator.";
    case "provider_error":
      return "The identity provider rejected the sign-in. Please try again.";
    default:
      return "Single sign-on failed. Please try again.";
  }
}
