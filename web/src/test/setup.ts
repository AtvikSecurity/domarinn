import "@testing-library/jest-dom/vitest";
import { beforeEach, vi } from "vitest";
import { setMockEnabled } from "@/mocks/handlers";
import { log } from "@/lib/logger";

// Tests always run against the in-memory fixture, regardless of env.
setMockEnabled(true);

// Keep vitest output pristine. The app logger is verbose in dev/test, and the
// client's failure-path tests exercise log.error/log.debug on purpose. Before
// every test, raise the threshold (drops debug/info/warn) and stub the one
// level a threshold can't gate — error. Raw console is left untouched so React's
// own warnings still surface, and the logger's own tests set their own level and
// console spies, so they keep asserting real emission.
beforeEach(() => {
  log.setLevel("error");
  vi.spyOn(log, "error").mockImplementation(() => {});
});

// jsdom has no matchMedia; the theme module needs it. The stub implements every
// MediaQueryList member the app touches, so it is assignable without a cast.
if (!window.matchMedia) {
  window.matchMedia = (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  });
}
