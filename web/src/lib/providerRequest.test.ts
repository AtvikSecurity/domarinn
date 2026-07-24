import { describe, expect, it } from "vitest";
import {
  formatJson,
  parseProviderRequest,
  promptAsJson,
  requestModel,
  requestTarget,
} from "./providerRequest";

describe("parseProviderRequest", () => {
  it("narrows the http envelope the openai/anthropic providers emit", () => {
    const view = parseProviderRequest({
      transport: "http",
      method: "POST",
      url: "https://gw.example/v1/chat/completions",
      body: { model: "qwen3.5", temperature: 0, messages: [] },
    });

    expect(view).toEqual({
      transport: "http",
      method: "POST",
      url: "https://gw.example/v1/chat/completions",
      payload: { model: "qwen3.5", temperature: 0, messages: [] },
    });
  });

  it("narrows the exec envelope and tolerates a missing args list", () => {
    const view = parseProviderRequest({
      transport: "exec",
      command: "./sut",
      stdin: { envelope: { protocol: 1 } },
    });
    expect(view).toMatchObject({
      transport: "exec",
      command: "./sut",
      args: [],
      payload: { envelope: { protocol: 1 } },
    });
  });

  it("drops non-string args rather than rendering `undefined` in a command line", () => {
    const view = parseProviderRequest({
      transport: "exec",
      command: "./sut",
      args: ["--mode", 7, null, "eval"],
    });
    expect(view).toMatchObject({ args: ["--mode", "eval"] });
  });

  it("falls back to `other` for an unrecognized transport, keeping the envelope", () => {
    const raw = { transport: "grpc", channel: "sut:9000" };
    expect(parseProviderRequest(raw)).toEqual({ transport: "other", payload: raw });
  });

  it("falls back to `other` when a known transport is missing its target", () => {
    // A malformed envelope must not render as an authoritative `POST undefined`.
    const raw = { transport: "http", body: { model: "m" } };
    expect(parseProviderRequest(raw)).toEqual({ transport: "other", payload: raw });
  });

  it("returns null when nothing was captured", () => {
    // The http provider withholds its request, and runs predating capture have
    // no field at all — both must read as "absent", not as an empty request.
    expect(parseProviderRequest(undefined)).toBeNull();
    expect(parseProviderRequest(null)).toBeNull();
    expect(parseProviderRequest("POST /v1/chat")).toBeNull();
    expect(parseProviderRequest([1, 2])).toBeNull();
  });
});

describe("requestTarget", () => {
  it("reads as a request line for http", () => {
    const view = parseProviderRequest({
      transport: "http",
      method: "POST",
      url: "https://a/v1/messages",
      body: {},
    })!;
    expect(requestTarget(view)).toBe("POST https://a/v1/messages");
  });

  it("defaults a missing method rather than printing `undefined`", () => {
    const view = parseProviderRequest({
      transport: "http",
      url: "https://a/v1/messages",
    })!;
    expect(requestTarget(view)).toBe("POST https://a/v1/messages");
  });

  it("reads as a command line for exec", () => {
    const view = parseProviderRequest({
      transport: "exec",
      command: "./sut",
      args: ["--mode", "eval"],
    })!;
    expect(requestTarget(view)).toBe("./sut --mode eval");
  });

  it("is empty for an unknown transport, which is shown whole instead", () => {
    expect(requestTarget({ transport: "other", payload: {} })).toBe("");
  });
});

describe("requestModel", () => {
  it("reports the model the payload names", () => {
    const view = parseProviderRequest({
      transport: "http",
      url: "u",
      body: { model: "claude-x" },
    })!;
    expect(requestModel(view)).toBe("claude-x");
  });

  it("is null when the payload names no model", () => {
    const view = parseProviderRequest({
      transport: "exec",
      command: "./sut",
      stdin: { vars: {} },
    })!;
    expect(requestModel(view)).toBeNull();
  });

  it("is null for a non-object payload", () => {
    const view = parseProviderRequest({ transport: "http", url: "u", body: "raw" })!;
    expect(requestModel(view)).toBeNull();
  });
});

describe("promptAsJson", () => {
  it("passes a rendered prompt through untouched", () => {
    const prompt = { messages: [{ role: "user" as const, content: "hi" }] };
    expect(promptAsJson(prompt)).toBe(prompt);
  });

  it("is null when there is no prompt", () => {
    expect(promptAsJson(undefined)).toBeNull();
  });
});

describe("formatJson", () => {
  it("pretty-prints with two-space indent", () => {
    expect(formatJson({ a: 1 })).toBe('{\n  "a": 1\n}');
  });

  it("degrades to a string instead of throwing on a cycle", () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(() => formatJson(cyclic)).not.toThrow();
  });

  it("degrades to a string for values JSON drops", () => {
    expect(formatJson(undefined)).toBe("undefined");
  });
});
