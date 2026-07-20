import { NavLink, Outlet } from "react-router";
import { useMeta } from "@/api/queries";
import { isMockEnabled } from "@/api/client";
import { cn } from "@/lib/cn";
import { ThemeToggleButton } from "./ThemeToggle";

const NAV = [
  { to: "/", label: "Runs", end: true },
  { to: "/cache", label: "Cache", end: false },
  { to: "/settings", label: "Settings", end: false },
];

function Logo() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden>
      <rect x="3" y="13" width="4" height="8" rx="1" fill="var(--color-accent)" />
      <rect x="10" y="8" width="4" height="13" rx="1" fill="var(--color-pass)" />
      <rect x="17" y="3" width="4" height="18" rx="1" fill="var(--color-accent)" opacity="0.6" />
    </svg>
  );
}

export function Layout() {
  const meta = useMeta();
  return (
    <div className="flex min-h-full flex-col">
      <header className="sticky top-0 z-30 border-b border-border bg-surface/85 backdrop-blur supports-[backdrop-filter]:bg-surface/70">
        <div className="mx-auto flex h-14 w-full max-w-[1600px] items-center gap-4 px-4 sm:px-6">
          <NavLink to="/" className="flex items-center gap-2 font-semibold">
            <Logo />
            <span className="tracking-tight">measurellm</span>
          </NavLink>
          <nav className="flex items-center gap-1" aria-label="Primary">
            {NAV.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.end}
                className={({ isActive }) =>
                  cn(
                    "rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
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
          <div className="ml-auto flex items-center gap-3">
            {isMockEnabled() ? (
              <span className="rounded-full bg-amber/12 px-2 py-0.5 text-[11px] font-medium text-amber ring-1 ring-inset ring-amber/25">
                mock data
              </span>
            ) : null}
            {meta.data ? (
              <span className="hidden text-xs text-muted sm:inline">
                v{meta.data.version} · {meta.data.auth_mode}
              </span>
            ) : null}
            <ThemeToggleButton />
          </div>
        </div>
      </header>
      <main className="mx-auto w-full max-w-[1600px] flex-1 px-4 py-6 sm:px-6">
        <Outlet />
      </main>
    </div>
  );
}
