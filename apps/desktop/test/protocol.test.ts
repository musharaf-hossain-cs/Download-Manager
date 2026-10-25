import { describe, expect, it } from "vitest";

import { createUiRequest } from "../src/protocol.js";

describe("UI protocol", () => {
  it("matches the engine health-request snapshot", () => {
    const message = createUiRequest("018f47ce-8e51-7f31-a1ec-4a97f84a12cd", {
      type: "health",
    });

    expect(JSON.stringify(message)).toBe(
      '{"version":1,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","payload":{"type":"health"}}',
    );
  });
});
