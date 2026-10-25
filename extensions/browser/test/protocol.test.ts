import { describe, expect, it } from "vitest";

import {
  MAX_EXTENSION_BATCH_ITEMS,
  encodeExtensionRequest,
} from "../src/protocol.js";

const base = {
  requestId: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
  nonce: "abcdefghijklmnop",
  extensionId: "packaged-extension-id",
} as const;

describe("extension protocol", () => {
  it("encodes the versioned native message envelope", () => {
    const encoded = encodeExtensionRequest({
      ...base,
      payload: { type: "health" },
    });

    expect(new TextDecoder().decode(encoded)).toBe(
      '{"version":1,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","nonce":"abcdefghijklmnop","extension_id":"packaged-extension-id","payload":{"type":"health"}}',
    );
  });

  it("rejects unsafe schemes, invalid nonces, and oversized batches", () => {
    expect(() =>
      encodeExtensionRequest({
        ...base,
        payload: {
          type: "add_download",
          url: "file:///secret",
          referrer: null,
          suggested_filename: null,
        },
      }),
    ).toThrow("HTTP(S)");

    expect(() =>
      encodeExtensionRequest({
        ...base,
        nonce: "short",
        payload: { type: "health" },
      }),
    ).toThrow("nonce");

    expect(() =>
      encodeExtensionRequest({
        ...base,
        payload: {
          type: "add_batch",
          items: Array.from({ length: MAX_EXTENSION_BATCH_ITEMS + 1 }, () => ({
            url: "https://example.test/file",
            referrer: null,
            suggested_filename: null,
          })),
        },
      }),
    ).toThrow("128 items");
  });
});
