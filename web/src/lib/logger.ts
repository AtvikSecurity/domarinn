// Tiny zero-dependency leveled logger for the web UI.
//
// Silent-by-default in prod (warn+error only, so user-reported console output
// still surfaces real problems) and verbose in dev. Raw `console.*` is banned
// everywhere else by eslint (see web/eslint.config.js); this module is the one
// exemption and the single place console access lives.

export type Level = "debug" | "info" | "warn" | "error";

const ORDER: Record<Level, number> = { debug: 10, info: 20, warn: 30, error: 40 };

/**
 * Pure level resolver, kept separate from the module's mutable state so it is
 * unit-testable without stubbing `import.meta.env`: an explicit, recognized
 * `VITE_LOG_LEVEL` always wins; otherwise dev builds default to `debug` and
 * prod builds to `warn` (warn + error only).
 */
export function resolveLevel(env: string | undefined, isDev: boolean): Level {
  if (env && env in ORDER) return env as Level;
  return isDev ? "debug" : "warn";
}

let threshold = ORDER[resolveLevel(import.meta.env.VITE_LOG_LEVEL, import.meta.env.DEV)];

// Sinks deref `console` at call time (not capture time) so a test's
// `vi.spyOn(console, ...)` is observed and so any late console shim still wins.
const SINKS: Record<Level, (...args: unknown[]) => void> = {
  debug: (...args: unknown[]) => console.debug(...args),
  info: (...args: unknown[]) => console.info(...args),
  warn: (...args: unknown[]) => console.warn(...args),
  error: (...args: unknown[]) => console.error(...args),
};

function emit(level: Level, ...args: unknown[]): void {
  if (ORDER[level] < threshold) return;
  SINKS[level]("[domarinn]", ...args);
}

export const log = {
  debug: (...args: unknown[]) => emit("debug", ...args),
  info: (...args: unknown[]) => emit("info", ...args),
  warn: (...args: unknown[]) => emit("warn", ...args),
  error: (...args: unknown[]) => emit("error", ...args),
  /** Override the runtime threshold (e.g. from a debug toggle). */
  setLevel(level: Level): void {
    threshold = ORDER[level];
  },
};
