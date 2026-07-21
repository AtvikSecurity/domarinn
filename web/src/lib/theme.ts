// Dark mode via the `class` strategy: a persisted toggle overrides the OS
// preference. The initial class is applied by an inline script in index.html to
// avoid a flash; this module keeps React in sync afterwards.

export type ThemePref = "light" | "dark" | "system";

const THEME_KEY = "domarinn.theme";

export function getStoredTheme(): ThemePref {
  try {
    const v = localStorage.getItem(THEME_KEY);
    if (v === "light" || v === "dark") return v;
  } catch {
    /* ignore */
  }
  return "system";
}

export function prefersDark(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-color-scheme: dark)").matches
  );
}

export function resolveIsDark(pref: ThemePref): boolean {
  if (pref === "dark") return true;
  if (pref === "light") return false;
  return prefersDark();
}

export function applyTheme(pref: ThemePref): void {
  const isDark = resolveIsDark(pref);
  document.documentElement.classList.toggle("dark", isDark);
}

export function storeTheme(pref: ThemePref): void {
  try {
    if (pref === "system") localStorage.removeItem(THEME_KEY);
    else localStorage.setItem(THEME_KEY, pref);
  } catch {
    /* ignore */
  }
  applyTheme(pref);
}
