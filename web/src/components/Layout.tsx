import { useEffect, useRef, useState } from "react";
import {
  NavLink,
  Navigate,
  Outlet,
  useLocation,
  useNavigate,
} from "react-router";
import { isMockEnabled } from "@/api/client";
import { useAuth } from "@/auth/AuthProvider";
import { onUnauthorized } from "@/lib/auth";
import { cn } from "@/lib/cn";
import { navItems } from "@/lib/nav";
import { Button } from "./ui/Button";
import { Chip } from "./ui/Chip";
import { DesktopNav } from "./DesktopNav";
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

  const publicPage = location.pathname === "/login" || location.pathname === "/setup";
  const nav = publicPage ? [] : navItems(view);
  const showWorkspace = nav.length > 0;

  return (
    // Pages such as run detail use the remaining height for their own grid.
    <div className="flex h-dvh flex-col overflow-hidden">
      <a href="#main-content" className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-surface focus:px-3 focus:py-2 focus:ring-2 focus:ring-ring">
        Skip to content
      </a>
      <header className="z-30 shrink-0 border-b border-border bg-bg">
        <div className="flex h-14 w-full items-center gap-3 px-4 sm:gap-4 sm:px-6">
          <NavLink
            to="/"
            aria-label="domarinn"
            className="flex shrink-0 items-center gap-2 font-semibold focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring md:w-36 lg:w-40"
          >
            <Logo />
            <span className="hidden text-sm tracking-tight min-[360px]:inline">domarinn</span>
          </NavLink>
          {showWorkspace && <SearchBar />}
          <div className="ml-auto flex shrink-0 items-center gap-2 sm:gap-3">
            {isMockEnabled() ? <Chip tone="amber">mock data</Chip> : null}
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
            {showWorkspace && (
              <MobileNavSheet nav={nav}>
                {(close) => <SearchBar variant="sheet" onNavigate={close} />}
              </MobileNavSheet>
            )}
          </div>
        </div>
      </header>
      <div className="flex min-h-0 flex-1">
        {showWorkspace && <DesktopNav nav={nav} />}
        <main
          id="main-content"
          tabIndex={-1}
          className={cn(
            "mx-auto w-full max-w-[1600px] min-h-0 min-w-0 flex-1 px-4 py-6 sm:px-6",
            // Filling pages manage their grid's scroll. On narrow screens,
            // the stacked header leaves too little height, so main scrolls.
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
    </div>
  );
}
