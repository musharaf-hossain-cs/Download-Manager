import { describe, expect, it } from "vitest";

import {
  evaluateCaptureRules,
  type CaptureRuleCandidate,
  type CaptureRuleConfig,
} from "../src/capture-rules.js";

const config: CaptureRuleConfig = {
  extensions: [".zip", "*.EXE"],
  mimeTypes: ["application/pdf", "video/*"],
  minimumSizeBytes: 1_024,
};

const candidate: CaptureRuleCandidate = {
  url: "https://files.example.test/download/archive.zip?token=one",
  suggestedFilename: null,
  mimeType: "application/octet-stream",
  sizeBytes: 4_096,
};

describe("capture MIME, extension, and minimum-size rules", () => {
  it.each([
    {
      name: "extension from the URL",
      input: candidate,
      expected: {
        capture: true,
        reason: "extension_match",
        matchedRule: "zip",
      },
    },
    {
      name: "suggested filename before the URL",
      input: { ...candidate, suggestedFilename: "setup.EXE" },
      expected: {
        capture: true,
        reason: "extension_match",
        matchedRule: "exe",
      },
    },
    {
      name: "exact MIME with parameters removed",
      input: {
        ...candidate,
        url: "https://files.example.test/download",
        mimeType: "application/pdf; charset=binary",
      },
      expected: {
        capture: true,
        reason: "mime_match",
        matchedRule: "application/pdf",
      },
    },
    {
      name: "MIME wildcard",
      input: {
        ...candidate,
        url: "https://files.example.test/stream",
        mimeType: "video/webm",
      },
      expected: {
        capture: true,
        reason: "mime_match",
        matchedRule: "video/*",
      },
    },
    {
      name: "known object below minimum",
      input: { ...candidate, sizeBytes: 1_023 },
      expected: { capture: false, reason: "below_minimum_size" },
    },
    {
      name: "unknown size can still match",
      input: { ...candidate, sizeBytes: null },
      expected: {
        capture: true,
        reason: "extension_match",
        matchedRule: "zip",
      },
    },
    {
      name: "no configured type match",
      input: {
        ...candidate,
        url: "https://files.example.test/readme.txt",
        mimeType: "text/plain",
      },
      expected: { capture: false, reason: "no_type_match" },
    },
    {
      name: "invalid size metadata",
      input: { ...candidate, sizeBytes: -1 },
      expected: { capture: false, reason: "invalid_candidate" },
    },
  ])("returns an explicit reason for $name", ({ input, expected }) => {
    expect(evaluateCaptureRules(input, config)).toEqual(expected);
  });

  it("rejects invalid configuration instead of silently weakening it", () => {
    expect(() =>
      evaluateCaptureRules(candidate, { ...config, minimumSizeBytes: -1 }),
    ).toThrow("minimum size");
    expect(() =>
      evaluateCaptureRules(candidate, { ...config, mimeTypes: ["video"] }),
    ).toThrow("MIME rule");
  });
});
