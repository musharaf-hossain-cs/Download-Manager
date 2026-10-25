import { describe, expect, it } from "vitest";

import { extensionIdentity } from "../src/index.js";

describe("browser extension package", () => {
  it("declares both milestone-zero browser families", () => {
    expect(extensionIdentity.supportedBrowsers).toEqual([
      "chromium",
      "firefox",
    ]);
    expect(extensionIdentity.version).toBe("0.1.0");
  });
});
