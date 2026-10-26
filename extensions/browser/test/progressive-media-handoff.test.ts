import { describe, expect, it, vi } from "vitest";

import type { MediaCandidate } from "../src/media-candidates.js";
import {
  ProgressiveMediaHandoffController,
  type ProgressiveMediaVariant,
} from "../src/progressive-media-handoff.js";
import type { ExtensionRequestEnvelope } from "../src/protocol.js";

const encoder = new TextEncoder();
const REFERRER = "https://watch.example.test/show/episode-1";
const context = {
  requestId: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
  nonce: "abcdefghijklmnop",
  extensionId: "abcdefghijklmnopabcdefghijklmnop",
} as const;

const fixtures = [
  {
    url: "https://cdn.example.test/media/movie.mp4?token=one",
    mimeType: "video/mp4",
    filename: "movie.mp4",
    bytes: encoder.encode("\0\0\0\x18ftypisomprogressive-mp4"),
    sha256: "9f920ba4f36163b05a562bcf1cb23f0dffd7c13c4163fb381a0890e9fb5ef73d",
  },
  {
    url: "https://cdn.example.test/media/movie.webm?token=two",
    mimeType: "video/webm",
    filename: "movie.webm",
    bytes: Uint8Array.from([
      0x1a,
      0x45,
      0xdf,
      0xa3,
      ...encoder.encode("progressive-webm"),
    ]),
    sha256: "781a43429928ba9bd54d9f89b1b64fa12ab6f35bfe59bc15732aabfff98cffae",
  },
  {
    url: "https://cdn.example.test/media/audio.mp3?token=three",
    mimeType: "audio/mpeg",
    filename: "audio.mp3",
    bytes: encoder.encode("ID3\x04\0\0progressive-audio"),
    sha256: "3c75b3899b310df2b4ec91fe73523b80b30c7d820a0aa28899d0383c0133b615",
  },
] as const;

function variant(
  fixture: (typeof fixtures)[number],
  referrer = `${REFERRER}#player`,
): ProgressiveMediaVariant {
  return {
    candidate: {
      url: fixture.url,
      kind: "progressive",
      mimeType: fixture.mimeType,
      playerIds: ["hero"],
      sources: ["media-element"],
    },
    referrer,
    suggestedFilename: fixture.filename,
  };
}

async function sha256(bytes: Uint8Array): Promise<string> {
  const owned = new Uint8Array(bytes.byteLength);
  owned.set(bytes);
  const digest = await crypto.subtle.digest("SHA-256", owned.buffer);
  return [...new Uint8Array(digest)]
    .map((value) => value.toString(16).padStart(2, "0"))
    .join("");
}

class RefererFixtureMessenger {
  readonly transfers: {
    readonly url: string;
    readonly referrer: string;
    readonly filename: string | null;
    readonly bytes: Uint8Array;
    readonly sha256: string;
  }[] = [];

  public async send(message: ExtensionRequestEnvelope): Promise<unknown> {
    const payload = message.payload;
    if (payload.type !== "add_download") throw new Error("wrong request type");
    const fixture = fixtures.find(({ url }) => url === payload.url);
    if (fixture === undefined || payload.referrer !== REFERRER) {
      return {
        version: 1,
        request_id: message.request_id,
        payload: {
          type: "rejected",
          code: "referer_required",
          message: "fixture requires the page referrer",
        },
      };
    }
    this.transfers.push({
      url: payload.url,
      referrer: payload.referrer,
      filename: payload.suggested_filename,
      bytes: fixture.bytes,
      sha256: await sha256(fixture.bytes),
    });
    return {
      version: 1,
      request_id: message.request_id,
      payload: {
        type: "accepted",
        job_ids: [`job-${String(this.transfers.length)}`],
      },
    };
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

describe("progressive media handoff", () => {
  it("hands MP4, WebM, and audio variants with required referrer and exact hashes", async () => {
    const messenger = new RefererFixtureMessenger();
    const controller = new ProgressiveMediaHandoffController(
      { state: "connected" },
      messenger,
      () => context,
    );

    for (const [index, fixture] of fixtures.entries()) {
      await expect(controller.handoff(variant(fixture))).resolves.toEqual({
        kind: "accepted",
        jobId: `job-${String(index + 1)}`,
      });
    }
    expect(messenger.transfers).toHaveLength(3);
    for (const [index, transfer] of messenger.transfers.entries()) {
      const fixture = fixtures[index];
      expect(fixture).toBeDefined();
      expect(transfer).toMatchObject({
        url: fixture?.url,
        referrer: REFERRER,
        filename: fixture?.filename,
        sha256: fixture?.sha256,
      });
      expect([...transfer.bytes]).toEqual([...(fixture?.bytes ?? [])]);
    }
  });

  it("surfaces a native referer rejection without recording fixture bytes", async () => {
    const messenger = new RefererFixtureMessenger();
    const controller = new ProgressiveMediaHandoffController(
      { state: "connected" },
      messenger,
      () => context,
    );

    await expect(
      controller.handoff(
        variant(fixtures[0], "https://other.example.test/page"),
      ),
    ).resolves.toEqual({ kind: "rejected", reason: "native_rejected" });
    expect(messenger.transfers).toEqual([]);
  });

  it("rejects unsafe, adaptive, malformed-filename, and offline variants before send", async () => {
    const send = vi.fn();
    const connected = new ProgressiveMediaHandoffController(
      { state: "connected" },
      { send },
      () => context,
    );
    const adaptive: MediaCandidate = {
      ...variant(fixtures[0]).candidate,
      url: "https://cdn.example.test/master.m3u8",
      kind: "hls",
      mimeType: "application/vnd.apple.mpegurl",
    };
    const cases: ProgressiveMediaVariant[] = [
      { ...variant(fixtures[0]), referrer: "file:///private/page.html" },
      { ...variant(fixtures[0]), candidate: adaptive },
      {
        ...variant(fixtures[0]),
        candidate: { ...variant(fixtures[0]).candidate, kind: "dash" },
      },
      { ...variant(fixtures[0]), suggestedFilename: "bad\rname.mp4" },
    ];
    for (const candidate of cases) {
      await expect(connected.handoff(candidate)).resolves.toEqual({
        kind: "ignored",
        reason: "ineligible",
      });
    }
    const offline = new ProgressiveMediaHandoffController(
      { state: "disconnected" },
      { send },
      () => context,
    );
    await expect(offline.handoff(variant(fixtures[0]))).resolves.toEqual({
      kind: "rejected",
      reason: "native_unavailable",
    });
    expect(send).not.toHaveBeenCalled();
  });

  it("deduplicates pending and accepted variants but permits retry after invalid ACK", async () => {
    const response = deferred<unknown>();
    const send = vi
      .fn()
      .mockReturnValueOnce(response.promise)
      .mockResolvedValueOnce({
        version: 1,
        request_id: context.requestId,
        payload: { type: "accepted", job_ids: [] },
      })
      .mockResolvedValueOnce({
        version: 1,
        request_id: context.requestId,
        payload: { type: "accepted", job_ids: ["retry-job"] },
      });
    const controller = new ProgressiveMediaHandoffController(
      { state: "connected" },
      { send },
      () => context,
    );
    const first = controller.handoff(variant(fixtures[0]));
    await expect(controller.handoff(variant(fixtures[0]))).resolves.toEqual({
      kind: "ignored",
      reason: "duplicate",
    });
    response.resolve({
      version: 1,
      request_id: context.requestId,
      payload: { type: "accepted", job_ids: ["first-job"] },
    });
    await expect(first).resolves.toEqual({
      kind: "accepted",
      jobId: "first-job",
    });
    await expect(controller.handoff(variant(fixtures[0]))).resolves.toEqual({
      kind: "ignored",
      reason: "duplicate",
    });

    const retryVariant = variant(fixtures[1]);
    await expect(controller.handoff(retryVariant)).resolves.toEqual({
      kind: "rejected",
      reason: "invalid_response",
    });
    await expect(controller.handoff(retryVariant)).resolves.toEqual({
      kind: "accepted",
      jobId: "retry-job",
    });
    expect(send).toHaveBeenCalledTimes(3);
  });

  it("returns a bounded timeout and allows the same variant to retry", async () => {
    vi.useFakeTimers();
    try {
      const send = vi
        .fn()
        .mockReturnValueOnce(new Promise(() => undefined))
        .mockResolvedValueOnce({
          version: 1,
          request_id: context.requestId,
          payload: { type: "accepted", job_ids: ["retry-after-timeout"] },
        });
      const controller = new ProgressiveMediaHandoffController(
        { state: "connected" },
        { send },
        () => context,
        250,
      );
      const timedOut = controller.handoff(variant(fixtures[2]));
      await vi.advanceTimersByTimeAsync(250);
      await expect(timedOut).resolves.toEqual({
        kind: "rejected",
        reason: "native_timeout",
      });
      await expect(controller.handoff(variant(fixtures[2]))).resolves.toEqual({
        kind: "accepted",
        jobId: "retry-after-timeout",
      });
    } finally {
      vi.useRealTimers();
    }
  });
});
