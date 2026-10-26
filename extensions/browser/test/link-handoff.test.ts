import { describe, expect, it, vi } from "vitest";

import {
  LinkHandoffController,
  type BrowserDownloadCandidate,
} from "../src/index.js";

const candidate: BrowserDownloadCandidate = {
  downloadId: 42,
  url: "https://files.example.test/releases/app.zip",
  referrer: "https://www.example.test/downloads",
  suggestedFilename: "app.zip",
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

describe("eligible link handoff", () => {
  it("creates one native job and cancels only after its accepted ACK", async () => {
    const events: string[] = [];
    const response = deferred<unknown>();
    const sent: unknown[] = [];
    const controller = new LinkHandoffController(
      { state: "connected" },
      {
        send(message) {
          events.push("native-send");
          sent.push(message);
          return response.promise;
        },
      },
      {
        async cancel(downloadId) {
          events.push(`browser-cancel:${downloadId}`);
        },
        async fallback() {
          events.push("browser-fallback");
        },
      },
      () => ({
        requestId: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
        nonce: "abcdefghijklmnop",
        extensionId: "abcdefghijklmnopabcdefghijklmnop",
      }),
    );

    const result = controller.handoff(candidate);
    await Promise.resolve();
    expect(events).toEqual(["native-send"]);
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({
      payload: {
        type: "add_download",
        url: candidate.url,
        suggested_filename: "app.zip",
      },
    });

    response.resolve({
      version: 1,
      request_id: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
      payload: {
        type: "accepted",
        job_ids: ["018f47ce-8e51-7f31-a1ec-4a97f84a12ce"],
      },
    });
    await expect(result).resolves.toEqual({
      kind: "accepted",
      jobId: "018f47ce-8e51-7f31-a1ec-4a97f84a12ce",
    });
    expect(events).toEqual(["native-send", "browser-cancel:42"]);

    await expect(controller.handoff(candidate)).resolves.toEqual({
      kind: "ignored",
      reason: "duplicate",
    });
    expect(sent).toHaveLength(1);
  });

  it("never sends or cancels an ineligible link", async () => {
    let sends = 0;
    let cancels = 0;
    const controller = new LinkHandoffController(
      { state: "connected" },
      {
        async send() {
          sends += 1;
          return null;
        },
      },
      {
        async cancel() {
          cancels += 1;
        },
        async fallback() {
          throw new Error("ineligible downloads must not fallback");
        },
      },
      () => ({ requestId: "id", nonce: "abcdefghijklmnop", extensionId: "id" }),
    );

    await expect(
      controller.handoff({
        ...candidate,
        url: "file:///C:/secret.txt",
      }),
    ).resolves.toEqual({ kind: "ignored", reason: "ineligible" });
    expect({ sends, cancels }).toEqual({ sends: 0, cancels: 0 });
  });

  it("does not cancel when native rejects or returns an invalid job count", async () => {
    const responses = [
      {
        version: 1,
        request_id: "request-1",
        payload: { type: "rejected", code: "denied", message: "no" },
      },
      {
        version: 1,
        request_id: "request-1",
        payload: { type: "accepted", job_ids: [] },
      },
    ];
    let cancels = 0;
    const fallbacks: unknown[] = [];
    for (const [index, nativeResponse] of responses.entries()) {
      const controller = new LinkHandoffController(
        { state: "connected" },
        {
          async send() {
            return nativeResponse;
          },
        },
        {
          async cancel() {
            cancels += 1;
          },
          async fallback(details) {
            fallbacks.push(details);
          },
        },
        () => ({
          requestId: "request-1",
          nonce: "abcdefghijklmnop",
          extensionId: "abcdefghijklmnopabcdefghijklmnop",
        }),
      );
      const result = await controller.handoff({
        ...candidate,
        downloadId: index,
      });
      expect(result.kind).toBe("fallback");
    }
    expect(cancels).toBe(0);
    expect(fallbacks).toEqual([
      expect.objectContaining({ downloadId: 0, reason: "native_rejected" }),
      expect.objectContaining({ downloadId: 1, reason: "invalid_response" }),
    ]);
  });

  it("falls back once with the original link when the app is offline", async () => {
    const fallbacks: unknown[] = [];
    const controller = new LinkHandoffController(
      { state: "disconnected" },
      {
        async send() {
          throw new Error("must not send while disconnected");
        },
      },
      {
        async cancel() {
          throw new Error("offline handoff must not cancel");
        },
        async fallback(details) {
          fallbacks.push(details);
        },
      },
      () => ({ requestId: "id", nonce: "abcdefghijklmnop", extensionId: "id" }),
    );

    await expect(controller.handoff(candidate)).resolves.toEqual({
      kind: "fallback",
      reason: "native_unavailable",
    });
    expect(fallbacks).toEqual([
      { downloadId: 42, url: candidate.url, reason: "native_unavailable" },
    ]);
  });

  it("falls back on a bounded native timeout without cancelling", async () => {
    vi.useFakeTimers();
    const fallbacks: unknown[] = [];
    try {
      const controller = new LinkHandoffController(
        { state: "connected" },
        { send: () => new Promise(() => undefined) },
        {
          async cancel() {
            throw new Error("timed out handoff must not cancel");
          },
          async fallback(details) {
            fallbacks.push(details);
          },
        },
        () => ({
          requestId: "request-1",
          nonce: "abcdefghijklmnop",
          extensionId: "abcdefghijklmnopabcdefghijklmnop",
        }),
        500,
      );
      const result = controller.handoff(candidate);
      await vi.advanceTimersByTimeAsync(500);
      await expect(result).resolves.toEqual({
        kind: "fallback",
        reason: "native_timeout",
      });
      expect(fallbacks).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
