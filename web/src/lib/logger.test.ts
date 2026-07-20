import { afterEach, describe, expect, it, vi } from "vitest";
import { log, resolveLevel } from "./logger";

afterEach(() => {
  vi.restoreAllMocks();
  // `threshold` is shared module state; reset it so tests don't leak level.
  log.setLevel("debug");
});

describe("resolveLevel", () => {
  it("honors a valid VITE_LOG_LEVEL over the build default", () => {
    expect(resolveLevel("info", true)).toBe("info");
    expect(resolveLevel("error", false)).toBe("error");
  });

  it("defaults to debug in dev and warn in prod when env is unset", () => {
    expect(resolveLevel(undefined, true)).toBe("debug");
    expect(resolveLevel(undefined, false)).toBe("warn");
  });

  it("ignores an empty or unrecognized level and uses the build default", () => {
    expect(resolveLevel("", true)).toBe("debug");
    expect(resolveLevel("verbose", false)).toBe("warn");
  });
});

describe("log emit gating", () => {
  it("suppresses below-threshold levels and prefixes emitted lines", () => {
    const debugSpy = vi.spyOn(console, "debug").mockImplementation(() => {});
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

    log.setLevel("warn");
    log.debug("nope", 1);
    log.warn("heads up", { a: 1 });

    expect(debugSpy).not.toHaveBeenCalled();
    expect(warnSpy).toHaveBeenCalledWith("[measurellm]", "heads up", { a: 1 });
  });

  it("setLevel lowers the threshold so debug is emitted again", () => {
    const debugSpy = vi.spyOn(console, "debug").mockImplementation(() => {});

    log.setLevel("warn");
    log.debug("still quiet");
    expect(debugSpy).not.toHaveBeenCalled();

    log.setLevel("debug");
    log.debug("now visible");
    expect(debugSpy).toHaveBeenCalledWith("[measurellm]", "now visible");
  });
});
