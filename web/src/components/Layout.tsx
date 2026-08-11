import { useEffect, useRef, useState } from "react";
import {
  NavLink,
  Navigate,
  Outlet,
  useLocation,
  useNavigate,
} from "react-router";
import { useMeta } from "@/api/queries";
import { isMockEnabled } from "@/api/client";
import { useAuth } from "@/auth/AuthProvider";
import { onUnauthorized } from "@/lib/auth";
import { cn } from "@/lib/cn";
import { navItems } from "@/lib/nav";
import { Button } from "./ui/Button";
import { Chip } from "./ui/Chip";
import { MobileNavSheet } from "./MobileNavSheet";
import { SearchBar } from "./SearchBar";
import { ThemeToggleButton } from "./ThemeToggle";
import { FillViewportProvider, useFillViewportState } from "./AppShell";

/**
 * Router-scoped bridge from the app-wide 401 signal to a login redirect.
 *
 * The 401 bus (`onUnauthorized`) lives outside React Router — it is fired from
 * the fetch wrapper and consumed by `AuthProvider`, which sits above
 * `RouterProvider`. This hook runs inside the router (Layout is the `/` route
 * element, mounted on every page), so it can turn an unhandled 401 into a
 * `navigate("/login")` in ANY auth mode — replacing the old token-paste modal.
 *
 * It complements, not duplicates, `RequireAuth`: that gate redirects when an
 * anonymous visitor lands directly on a closed-mode route (no request has 401'd
 * yet); this hook redirects when a request actually comes back 401 (an expired
 * session, or a protected action in open/protect-writes mode). The attempted
 * location rides along as `from` so login can restore the deep link.
 */
function useUnauthorizedRedirect(): void {
  const navigate = useNavigate();
  const location = useLocation();
  // Mirror the live location into a ref so the once-registered listener always
  // sees the current page without re-subscribing on every navigation.
  const locationRef = useRef(location);
  useEffect(() => {
    locationRef.current = location;
  }, [location]);

  useEffect(
    () =>
      onUnauthorized(() => {
        const current = locationRef.current;
        // Already on a public auth page — nothing to redirect to (and their
        // own requests never 401 anyway), so avoid a self-navigation loop.
        if (current.pathname === "/login" || current.pathname === "/setup") {
          return;
        }
        void navigate("/login", { replace: true, state: { from: current } });
      }),
    [navigate],
  );
}

function Logo() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden>
      <rect x="3" y="13" width="4" height="8" rx="1" fill="var(--color-accent)" />
      <rect x="10" y="8" width="4" height="13" rx="1" fill="var(--color-pass)" />
      <rect x="17" y="3" width="4" height="18" rx="1" fill="var(--color-accent)" opacity="0.6" />
    </svg>
  );
}

/** The signed-in user's permission role (admin / viewer). */
function AuthRoleChip({ role }: { role: string }) {
  return (
    <Chip tone={role === "admin" ? "accent" : "neutral"} size="xs">
      {role}
    </Chip>
  );
}

function LogoutButton() {
  const { logout } = useAuth();
  const [pending, setPending] = useState(false);
  return (
    <Button
      variant="ghost"
      size="sm"
      disabled={pending}
      onClick={async () => {
        setPending(true);
        try {
          await logout();
        } finally {
          setPending(false);
        }
      }}
    >
      {pending ? "…" : "Log out"}
    </Button>
  );
}

export function Layout() {
  const meta = useMeta();
  const { view } = useAuth();
  const location = useLocation();
  // A page can claim the shell's remaining height instead of scrolling in it.
  const { fill, setFill } = useFillViewportState();

  // Turn any unhandled 401 into a redirect to /login (all auth modes).
  useUnauthorizedRedirect();

  // First-run gate: force the setup flow until an admin exists.
  if (view.setupRequired && location.pathname !== "/setup") {
    return <Navigate to="/setup" replace />;
  }

  // Closed mode + anonymous: the only reachable page is /login, so a full nav
  // would show dead links that bounce straight back. Render a bare header.
  const chromeOnly = view.needsLogin;

  const nav = navItems(view);

  return (
    // `h-dvh` + a non-scrolling root: the shell owns the viewport, so a page
    // that fills it (see `useFillViewport`) can hand its own scroller the exact
    // remaining height instead of nesting one scrollport inside another.
    <div className="flex h-dvh flex-col overflow-hidden">
      {/* Same fill as the page body. The bar is a flex sibling of `<main>`, not
          an overlay, so nothing ever scrolls beneath it — the translucency and
          `backdrop-blur` this used to carry had nothing to act on and only
          composited `--surface` against `--bg`. */}
      <header className="z-30 shrink-0 border-b border-border bg-bg">
        {/* `min-w-0` on the flex children plus a scrollable nav: with neither,
            the nav and the right-hand cluster refused to shrink and pushed the
            document to 508px wide at a 390px viewport — the one place the page
            body itself scrolled sideways. Still load-bearing between `md` and
            a narrow desktop; below `md` the two elements that were fighting
            for that width are no longer rendered at all, and the menu sheet
            carries them instead. */}
        <div className="mx-auto flex h-14 w-full max-w-[1600px] items-center gap-3 px-4 sm:gap-4 sm:px-6">
          <NavLink
            to="/"
            className="flex shrink-0 items-center gap-2 font-semibold"
          >
            <Logo />
            <span className="hidden tracking-tight sm:inline">domarinn</span>
          </NavLink>
          {/* A horizontally scrolling strip has no affordance saying it
              scrolls, so on a phone the later items — API keys, Admin,
              Settings — simply vanished off the right edge. Below `md` the
              menu sheet owns navigation instead. */}
          <nav
            className="hidden min-w-0 items-center gap-1 overflow-x-auto md:flex"
            aria-label="Primary"
          >
            {nav.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.end}
                className={({ isActive }) =>
                  cn(
                    "shrink-0 whitespace-nowrap rounded-md px-2 py-1.5 text-sm font-medium transition-colors sm:px-3",
                    isActive
                      ? "bg-surface-2 text-fg"
                      : "text-muted hover:bg-surface-2 hover:text-fg",
                  )
                }
              >
                {item.label}
              </NavLink>
            ))}
          </nav>
          {chromeOnly ? null : <SearchBar />}
          <div className="ml-auto flex shrink-0 items-center gap-2 sm:gap-3">
            {isMockEnabled() ? <Chip tone="amber">mock data</Chip> : null}
            {meta.data ? (
              <span className="hidden text-xs text-muted sm:inline">
                v{meta.data.version} · {meta.data.auth_mode}
              </span>
            ) : null}
            {view.authenticated ? (
              <div className="flex items-center gap-2">
                <span className="hidden items-center gap-1.5 text-sm sm:flex">
                  <span className="font-medium text-fg">
                    {view.user?.username ?? "user"}
                  </span>
                  {view.role ? <AuthRoleChip role={view.role} /> : null}
                </span>
                {view.hasRealSession ? (
                  <LogoutButton />
                ) : (
                  <NavLink
                    to="/login"
                    className="rounded-md px-2 py-1 text-sm font-medium text-muted hover:text-fg"
                  >
                    Sign in
                  </NavLink>
                )}
              </div>
            ) : (
              <NavLink
                to="/login"
                className="rounded-md px-3 py-1.5 text-sm font-medium text-accent hover:underline"
              >
                Log in
              </NavLink>
            )}
            <ThemeToggleButton />
            {chromeOnly ? null : (
              // Search is the bigger phone gap than the nav strip was: the
              // header bar is `hidden md:flex`, so below `md` there was no way
              // to search at all. The header instance stays mounted but can
              // never be focused, so it never opens its panel and the two
              // cannot both claim the results element's id.
              <MobileNavSheet nav={nav}>
                {(close) => <SearchBar variant="sheet" onNavigate={close} />}
              </MobileNavSheet>
            )}
          </div>
        </div>
      </header>
      <main
        className={cn(
          "mx-auto w-full max-w-[1600px] min-h-0 flex-1 px-4 py-6 sm:px-6",
          // Filling pages manage their own scrolling; everything else scrolls
          // here, in exactly one place. Below `lg` the fill is dropped: the
          // run header stacks into three rows of tiles there and would leave
          // the grid no height, with no page scroll to reach it.
          fill
            ? "overflow-y-auto lg:flex lg:flex-col lg:overflow-hidden"
            : "overflow-y-auto",
        )}
      >
        <FillViewportProvider value={setFill}>
          <Outlet />
        </FillViewportProvider>
      </main>
    </div>
  );
}
