import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "typescript-eslint";

export default tseslint.config(
  // `src/api/generated` is ts-rs output (read-only, CI drift-checked); it is not
  // ours to lint and can emit constructs eslint dislikes (e.g. an `unknown` union
  // member in JsonValue).
  { ignores: ["dist", "coverage", "src/api/generated"] },
  {
    // App + build config: fully type-checked. These files are all covered by
    // the tsconfig project graph (src -> tsconfig.app, vite.config -> tsconfig.node),
    // so the project service can hand eslint real type information.
    extends: [js.configs.recommended, ...tseslint.configs.recommendedTypeChecked],
    files: ["src/**/*.{ts,tsx}", "vite.config.ts"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // Advisory for React Compiler adoption ("this library skips memoization
      // compilation"); we don't use the compiler, so it's pure noise here.
      "react-hooks/incompatible-library": "off",
      // HMR-only nicety; not worth splitting hook/component files for.
      "react-refresh/only-export-components": "off",
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      // React event handlers legitimately accept async functions; React invokes
      // them without awaiting, which is the intended fire-and-forget behavior.
      // Disable ONLY the JSX-attribute void-return check; the rule stays on for
      // every other void-return misuse (e.g. an async callback passed to forEach).
      "@typescript-eslint/no-misused-promises": [
        "error",
        { checksVoidReturn: { attributes: false } },
      ],
      // Route all logging through src/lib/logger.ts (leveled, silent-by-default
      // in prod); raw console.* is banned everywhere but that one module.
      "no-console": "error",
    },
  },
  {
    // Playwright e2e specs + their config live outside the app's tsconfig
    // project graph (Playwright compiles them itself), so type-checked linting
    // has no program to attach to. Keep them on the untyped recommended set.
    // The docs-screenshot pipeline (screenshots/**, its config, and the shared
    // chrome-resolution helper both configs import) is Playwright too.
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: [
      "e2e/**/*.ts",
      "playwright.config.ts",
      "playwright.shared.ts",
      "playwright.screenshots.config.ts",
      "screenshots/**/*.ts",
    ],
    languageOptions: {
      ecmaVersion: 2022,
      globals: { ...globals.browser, ...globals.node },
    },
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "no-console": "error",
    },
  },
  {
    // The logger is the single sanctioned home for raw console access.
    files: ["src/lib/logger.ts"],
    rules: {
      "no-console": "off",
    },
  },
  {
    files: ["**/*.{test,spec}.{ts,tsx}", "src/test/**", "src/mocks/**"],
    languageOptions: {
      globals: { ...globals.browser, ...globals.node },
    },
    rules: {
      "no-console": "off",
    },
  },
);
