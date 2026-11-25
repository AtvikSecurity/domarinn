import { useState } from "react";
import { NavLink } from "react-router";
import { cn } from "@/lib/cn";
import type { NavItem } from "@/lib/nav";
import { Tooltip } from "./ui/Tooltip";

const PREF = "domarinn.sidebar.compact";
const ICONS: Record<string, string> = {
  "/": "M3 3h7v7H3z M14 3h7v7h-7z M3 14h7v7H3z M14 14h7v7h-7z",
  "/runs": "M2 12h5l3-7 4 14 3-7h5",
  "/sets": "M3 7V5a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z",
  "/cache": "M20 6c0 2-3.6 3.5-8 3.5S4 8 4 6s3.6-3.5 8-3.5S20 4 20 6z M4 6v12c0 2 3.6 3.5 8 3.5s8-1.5 8-3.5V6 M4 12c0 2 3.6 3.5 8 3.5s8-1.5 8-3.5",
  "/keys": "M21 3l-6.5 6.5m3-3 2 2m-4-4 2 2M15 14a5.5 5.5 0 1 1-5.5-5.5 5.5 5.5 0 0 1 5.5 5.5z",
  "/admin": "M12 22s7-3.5 7-10V5l-7-3-7 3v7c0 6.5 7 10 7 10z M9 12l2 2 4-4",
  "/settings": "M4 6h5m4 0h7M4 12h10m4 0h2M4 18h2m4 0h10 M9 4v4m5 2v4m-8 2v4",
};

function NavIcon({ path }: { path: string }) {
  return (
    <svg className="size-[18px] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={path} />
    </svg>
  );
}

export function DesktopNav({ nav }: { nav: NavItem[] }) {
  const [compact, setCompact] = useState(() => {
    try { return localStorage.getItem(PREF) === "true"; } catch { return false; }
  });
  const toggleLabel = compact ? "Expand sidebar" : "Collapse sidebar";

  function toggle() {
    setCompact(!compact);
    try { localStorage.setItem(PREF, String(!compact)); } catch { /* Still usable in this tab. */ }
  }

  function renderLink(item: NavItem) {
    const link = (
      <NavLink
        to={item.to}
        end={item.end}
        className={cn(
          "flex min-h-10 items-center gap-2.5 rounded-md border border-transparent text-sm font-medium text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring aria-[current=page]:border-border aria-[current=page]:bg-surface-2 aria-[current=page]:text-fg",
          compact ? "justify-center" : "px-2.5",
        )}
      >
        <NavIcon path={ICONS[item.to] ?? "M5 4h14v16H5z M8 9h8m-8 4h8"} />
        <span className={compact ? "sr-only" : undefined}>{item.label}</span>
      </NavLink>
    );
    return compact ? <Tooltip key={item.to} content={item.label} side="right">{link}</Tooltip> : <span key={item.to}>{link}</span>;
  }

  return (
    <nav id="desktop-nav" aria-label="Primary" className={cn("hidden shrink-0 flex-col gap-4 overflow-y-auto border-r border-border bg-surface p-2.5 md:flex", compact ? "w-16" : "w-44 lg:w-48")}>
      <div className="grid gap-1 pt-1">{nav.filter((item) => !item.utility).map(renderLink)}</div>
      <div className="grid gap-1 border-t border-border pt-3">{nav.filter((item) => item.utility).map(renderLink)}</div>
      <Tooltip content={toggleLabel} side="right">
        <button
          type="button"
          aria-label={toggleLabel}
          onClick={toggle}
          className={cn("mt-auto flex min-h-10 shrink-0 items-center gap-2.5 rounded-md px-2.5 text-xs font-medium text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring", compact && "justify-center px-0")}
        >
          <NavIcon path={compact ? "M3 4h18v16H3z M9 4v16m4-10 2 2-2 2" : "M3 4h18v16H3z M9 4v16m6-10-2 2 2 2"} />
          {!compact && <span>Collapse</span>}
        </button>
      </Tooltip>
    </nav>
  );
}
