import { describe, expect, it } from "vitest";

import { createUiRequest, decodeUiMessage } from "../src/protocol.js";
import { jobFixture } from "./fixtures.js";

describe("UI protocol", () => {
  it("matches the engine health-request snapshot", () => {
    const message = createUiRequest("018f47ce-8e51-7f31-a1ec-4a97f84a12cd", {
      type: "health",
    });

    expect(JSON.stringify(message)).toBe(
      '{"version":1,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","payload":{"type":"health"}}',
    );
  });

  it("accepts a strict persisted-job response and rejects extra fields", () => {
    const response = {
      version: 1,
      request_id: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
      payload: {
        type: "response",
        response: { type: "jobs", jobs: [jobFixture()] },
      },
    };
    expect(decodeUiMessage(response)).toEqual(response);
    expect(() => decodeUiMessage({ ...response, unexpected: true })).toThrow(
      "invalid engine message envelope",
    );
  });
});
