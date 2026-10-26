import { describe, expect, it } from "vitest";

import { normalizeDropInputs } from "../src/drag-drop.js";

describe("drag-and-drop URL normalization", () => {
  it("normalizes URL, multiline text, and list inputs in encounter order", () => {
    expect(
      normalizeDropInputs([
        { kind: "url", value: " HTTPS://EXAMPLE.TEST/a " },
        {
          kind: "text",
          value: "https://example.test/b\r\n\r\nhttps://example.test/বাংলা",
        },
        {
          kind: "list",
          values: ["https://example.test/c", "https://example.test/d"],
        },
      ]).urls,
    ).toEqual([
      "https://example.test/a",
      "https://example.test/b",
      "https://example.test/%E0%A6%AC%E0%A6%BE%E0%A6%82%E0%A6%B2%E0%A6%BE",
      "https://example.test/c",
      "https://example.test/d",
    ]);
  });

  it("reports invalid and unsupported items with their source positions", () => {
    const report = normalizeDropInputs([
      { kind: "text", value: "not a URL\nfile:///C:/private.txt" },
      { kind: "list", values: ["", "ftp://example.test/archive.zip"] },
    ]);

    expect(report.urls).toEqual([]);
    expect(report.invalid).toEqual([
      {
        inputIndex: 0,
        itemIndex: 0,
        value: "not a URL",
        reason: "invalid_url",
      },
      {
        inputIndex: 0,
        itemIndex: 1,
        value: "file:///C:/private.txt",
        reason: "unsupported_scheme",
      },
      { inputIndex: 1, itemIndex: 0, value: "", reason: "invalid_url" },
      {
        inputIndex: 1,
        itemIndex: 1,
        value: "ftp://example.test/archive.zip",
        reason: "unsupported_scheme",
      },
    ]);
  });

  it("keeps the first normalized URL and reports later duplicates", () => {
    const report = normalizeDropInputs([
      { kind: "url", value: "https://EXAMPLE.test/file" },
      {
        kind: "list",
        values: ["https://example.test/file", "https://example.test/another"],
      },
    ]);

    expect(report.urls).toEqual([
      "https://example.test/file",
      "https://example.test/another",
    ]);
    expect(report.duplicates).toEqual(["https://example.test/file"]);
  });
});
