import { describe, expect, it } from "vitest";

import {
  evaluateSiteCapture,
  type SiteCaptureConfig,
} from "../src/site-rules.js";

describe("global and per-site capture rules", () => {
  it.each([
    {
      name: "global allow when no site matches",
      url: "https://unlisted.test/file.zip",
      config: { globalPolicy: "allow", sites: [] },
      expected: { capture: true, reason: "global_allow" },
    },
    {
      name: "global deny when no site matches",
      url: "https://unlisted.test/file.zip",
      config: { globalPolicy: "deny", sites: [] },
      expected: { capture: false, reason: "global_deny" },
    },
    {
      name: "wildcard site allow overrides global deny",
      url: "https://cdn.example.test/file.zip",
      config: {
        globalPolicy: "deny",
        sites: [{ hostPattern: "*.example.test", policy: "allow" }],
      },
      expected: {
        capture: true,
        reason: "site_allow",
        matchedPattern: "*.example.test",
      },
    },
    {
      name: "exact site deny beats wildcard allow",
      url: "https://private.example.test/file.zip",
      config: {
        globalPolicy: "allow",
        sites: [
          { hostPattern: "*.example.test", policy: "allow" },
          { hostPattern: "private.example.test", policy: "deny" },
        ],
      },
      expected: {
        capture: false,
        reason: "site_deny",
        matchedPattern: "private.example.test",
      },
    },
    {
      name: "more-specific wildcard wins",
      url: "https://cdn.downloads.example.test/file.zip",
      config: {
        globalPolicy: "deny",
        sites: [
          { hostPattern: "*.example.test", policy: "deny" },
          { hostPattern: "*.downloads.example.test", policy: "allow" },
        ],
      },
      expected: {
        capture: true,
        reason: "site_allow",
        matchedPattern: "*.downloads.example.test",
      },
    },
    {
      name: "deny wins equally specific conflicts",
      url: "https://files.example.test/file.zip",
      config: {
        globalPolicy: "allow",
        sites: [
          { hostPattern: "files.example.test", policy: "allow" },
          { hostPattern: "FILES.EXAMPLE.TEST", policy: "deny" },
        ],
      },
      expected: {
        capture: false,
        reason: "site_deny",
        matchedPattern: "files.example.test",
      },
    },
  ])("applies precedence for $name", ({ url, config, expected }) => {
    expect(evaluateSiteCapture(url, config as SiteCaptureConfig)).toEqual(
      expected,
    );
  });

  it("does not let a subdomain wildcard accidentally match the apex", () => {
    expect(
      evaluateSiteCapture("https://example.test/file.zip", {
        globalPolicy: "deny",
        sites: [{ hostPattern: "*.example.test", policy: "allow" }],
      }),
    ).toEqual({ capture: false, reason: "global_deny" });
  });

  it("rejects unsafe URLs and malformed patterns", () => {
    expect(
      evaluateSiteCapture("file:///C:/private.zip", {
        globalPolicy: "allow",
        sites: [],
      }),
    ).toEqual({ capture: false, reason: "invalid_site_url" });
    expect(() =>
      evaluateSiteCapture("https://example.test/file.zip", {
        globalPolicy: "allow",
        sites: [{ hostPattern: "*example.test", policy: "deny" }],
      }),
    ).toThrow("site host pattern");
  });
});
