import { describe, expect, it } from "vitest";

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
      expect(result.kind).toBe("rejected");
    }
    expect(cancels).toBe(0);
  });
});
