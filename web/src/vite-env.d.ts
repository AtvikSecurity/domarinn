/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Set to "1" to serve the in-memory fixture instead of the real API. */
  readonly VITE_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
