import "@testing-library/jest-dom/vitest";
import { setMockEnabled } from "@/mocks/handlers";

// Tests always run against the in-memory fixture, regardless of env.
setMockEnabled(true);

// jsdom has no matchMedia; the theme module needs it.
if (!window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}
