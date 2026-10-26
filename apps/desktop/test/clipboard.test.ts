import { describe, expect, it } from "vitest";

import { ClipboardUrlDetector } from "../src/clipboard.js";

describe("clipboard URL detection", () => {
  it("is optional and can be enabled without replaying disabled content", () => {
    const detector = new ClipboardUrlDetector();
    expect(detector.inspect("https://example.test/file.zip", 1_000)).toEqual({
      kind: "ignored",
      reason: "disabled",
    });

    detector.setEnabled(true);
    expect(detector.inspect("https://example.test/file.zip", 1_001)).toEqual({
      kind: "prompt",
      url: "https://example.test/file.zip",
    });
  });

  it("debounces a normalized duplicate and permits it after the window", () => {
    const detector = new ClipboardUrlDetector({
      enabled: true,
      debounceMs: 2_000,
    });
    expect(detector.inspect(" HTTPS://EXAMPLE.TEST/file.zip ", 10_000)).toEqual(
      {
        kind: "prompt",
        url: "https://example.test/file.zip",
      },
    );
    expect(detector.inspect("https://example.test/file.zip", 11_999)).toEqual({
      kind: "ignored",
      reason: "duplicate",
    });
    expect(detector.inspect("https://example.test/file.zip", 12_000)).toEqual({
      kind: "prompt",
      url: "https://example.test/file.zip",
    });
  });

  it.each(["", "notes only", "file:///C:/private.txt", "ftp://example.test/a"])(
    "ignores invalid or unsupported clipboard content: %s",
    (content) => {
      const detector = new ClipboardUrlDetector({ enabled: true });
      expect(detector.inspect(content, 1)).toEqual({
        kind: "ignored",
        reason: "invalid",
      });
    },
  );
});
