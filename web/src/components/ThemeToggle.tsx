import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { applyTheme, getStoredTheme, storeTheme, type ThemePref } from "@/lib/theme";
import { cn } from "@/lib/cn";

/** Shared theme state; keeps the OS `system` option live-tracking. */
export function useTheme() {
  const [pref, setPref] = useState<ThemePref>(() => getStoredTheme());

  useEffect(() => {
    applyTheme(pref);
    if (pref !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => applyTheme("system");
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [pref]);

  const set = useCallback((next: ThemePref) => {
    storeTheme(next);
    setPref(next);
  }, []);

  return { pref, set };
}

const SunIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
  </svg>
);
const MoonIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
  </svg>
);
const MonitorIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <rect x="3" y="4" width="18" height="12" rx="2" />
    <path d="M8 20h8M12 16v4" />
  </svg>
);

/** Compact icon toggle for the nav (cycles light <-> dark). */
export function ThemeToggleButton() {
  const { pref, set } = useTheme();
  const isDark = pref === "dark" || (pref === "system" && document.documentElement.classList.contains("dark"));
  return (
    <button
      type="button"
      onClick={() => set(isDark ? "light" : "dark")}
      className="grid size-8 place-items-center rounded-md text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      aria-label={isDark ? "Switch to light mode" : "Switch to dark mode"}
      title="Toggle theme"
    >
      {isDark ? <SunIcon /> : <MoonIcon />}
    </button>
  );
}

const OPTIONS: { value: ThemePref; label: string; icon: ReactNode }[] = [
  { value: "light", label: "Light", icon: <SunIcon /> },
  { value: "dark", label: "Dark", icon: <MoonIcon /> },
  { value: "system", label: "System", icon: <MonitorIcon /> },
];

/**
 * Full segmented control for the settings page.
 *
 * It claims the `radiogroup` role, so it behaves like one (mirroring
 * `ui/SegmentedControl`, whose underline styling does not fit here): a roving
 * tabindex makes the group a single Tab stop, and Arrow keys / Home / End move
 * the selection and focus together.
 */
export function ThemeSegmented() {
  const { pref, set } = useTheme();
  const groupRef = useRef<HTMLDivElement>(null);
  const activeIndex = Math.max(
    0,
    OPTIONS.findIndex((o) => o.value === pref),
  );

  function select(index: number) {
    const next = OPTIONS[(index + OPTIONS.length) % OPTIONS.length];
    if (!next) return;
    set(next.value);
    groupRef.current
      ?.querySelector<HTMLButtonElement>(`[data-value="${next.value}"]`)
      ?.focus();
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    switch (e.key) {
      case "ArrowRight":
      case "ArrowDown":
        e.preventDefault();
        select(activeIndex + 1);
        break;
      case "ArrowLeft":
      case "ArrowUp":
        e.preventDefault();
        select(activeIndex - 1);
        break;
      case "Home":
        e.preventDefault();
        select(0);
        break;
      case "End":
        e.preventDefault();
        select(OPTIONS.length - 1);
        break;
    }
  }

  return (
    <div
      ref={groupRef}
      role="radiogroup"
      aria-label="Theme"
      onKeyDown={onKeyDown}
      className="inline-flex rounded-lg border border-border bg-surface p-0.5"
    >
      {OPTIONS.map((opt, i) => (
        <button
          key={opt.value}
          type="button"
          role="radio"
          data-value={opt.value}
          aria-checked={pref === opt.value}
          tabIndex={i === activeIndex ? 0 : -1}
          onClick={() => set(opt.value)}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            pref === opt.value
              ? "bg-surface-2 text-fg shadow-sm"
              : "text-muted hover:text-fg",
          )}
        >
          {opt.icon}
          {opt.label}
        </button>
      ))}
    </div>
  );
}
