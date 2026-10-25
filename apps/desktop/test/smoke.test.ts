import { describe, expect, it } from "vitest";

import { desktopIdentity } from "../src/index.js";

describe("desktop package", () => {
  it("exports its stable bootstrap identity", () => {
    expect(desktopIdentity).toEqual({
      name: "download-manager-desktop",
      version: "0.1.0",
    });
  });
});
