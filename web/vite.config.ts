import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// Build output goes to web/dist so the Rust server can embed it via rust-embed.
// The dev server proxies /api to the local domarinn server (default port 8321).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: "http://localhost:8321",
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // Lazy chunks (jsdiff on the compare page) benefit from a slightly larger
    // warning limit; keep the app chunk lean otherwise.
    chunkSizeWarningLimit: 900,
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    // Vitest defaults to one worker per core, and each worker builds a full
    // jsdom — the profile puts environment setup an order of magnitude above
    // the tests themselves. That is free on a dev box, but CI shares one
    // self-hosted runner with the concurrent cargo jobs, and the unbounded fan
    // out exhausted it: the `web-test` step never reached a conclusion, the job
    // was killed after ~14 minutes, and no log blob was ever uploaded.
    //
    // Capping workers under CI trades a little wall-clock for a run that fits
    // in the machine it actually runs on. Local runs are untouched.
    ...(process.env.CI ? { maxWorkers: 2, minWorkers: 1 } : {}),
  },
});
