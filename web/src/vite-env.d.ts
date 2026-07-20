/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Set to "1" to serve the in-memory fixture instead of the real API. */
  readonly VITE_MOCK?: string;
  /** Overrides the logger's default threshold (dev: debug, prod: warn). */
  readonly VITE_LOG_LEVEL?: "debug" | "info" | "warn" | "error";
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
