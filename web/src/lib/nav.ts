import type { AuthView } from "./authz";

export interface NavItem {
  to: string;
  label: string;
  /** Match the path exactly. Only "/" needs it, since every route starts with it. */
  end?: boolean;
}

/**
 * The primary navigation, for whoever is looking at it.
 *
 * Extracted from the header so the header strip and the phone menu render the
 * same list rather than two lists that agree today. Pure, so the gating below
 * is testable without mounting the shell.
 */
export function navItems(view: AuthView): NavItem[] {
  // Closed mode with an anonymous visitor: /login is the only reachable page,
  // so a nav would be entirely dead links that bounce straight back.
  if (view.needsLogin) return [];

  const items: NavItem[] = [
    { to: "/", label: "Overview", end: true },
    { to: "/runs", label: "Runs" },
    { to: "/sets", label: "Sets" },
    { to: "/cache", label: "Cache" },
  ];
  // Keys are per-identity; offering them to someone who has not signed in
  // would show a page that can only tell them to sign in.
  if (!view.promptLogin) items.push({ to: "/keys", label: "API keys" });
  if (view.canAdmin) items.push({ to: "/admin", label: "Admin" });
  items.push({ to: "/settings", label: "Settings" });
  return items;
}
