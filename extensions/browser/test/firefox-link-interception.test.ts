import { describe, expect, it, vi } from "vitest";

import {
  FirefoxLinkInterception,
  type FirefoxDownloadItem,
  type FirefoxDownloadsApi,
} from "../src/index.js";

const firefoxExtensionId = "download-manager@musharaf-hossain-cs.github.io";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

function response(requestId: string, type: "accepted" | "rejected") {
  return type === "accepted"
    ? {
        version: 1,
        request_id: requestId,
        payload: { type, job_ids: ["018f47ce-8e51-7f31-a1ec-4a97f84a12ce"] },
      }
    : {
        version: 1,
        request_id: requestId,
        payload: { type, code: "denied", message: "not accepted" },
      };
}

function downloadsFixture(events: string[] = []) {
  let listener: ((item: FirefoxDownloadItem) => void) | undefined;
  const api: FirefoxDownloadsApi = {
    onCreated: {
      addListener(next) {
        listener = next;
        events.push("listener:add");
      },
      removeListener(next) {
        if (listener === next) listener = undefined;
        events.push("listener:remove");
      },
    },
    async cancel(downloadId) {
      events.push(`cancel:${downloadId}`);
    },
  };
  return { api, listener: () => listener };
}

function context(requestId = "018f47ce-8e51-7f31-a1ec-4a97f84a12cd") {
  return {
    requestId,
    nonce: "abcdefghijklmnop",
    extensionId: firefoxExtensionId,
  };
}

describe("Firefox link interception", () => {
  const browserFixtures: readonly {
    readonly name: string;
    readonly item: FirefoxDownloadItem;
    readonly expectedReferrer: string | null;
    readonly expectedFilename: string | null;
  }[] = [
    {
      name: "stable",
      item: {
        id: 42,
        url: "https://files.example.test/releases/app.zip",
        filename: "C:\\Users\\Person\\Downloads\\app.zip",
        referrer: "https://www.example.test/downloads",
      },
      expectedReferrer: "https://www.example.test/downloads",
      expectedFilename: "app.zip",
    },
    {
      name: "ESR",
      item: {
        id: 43,
        url: "https://files.example.test/releases/archive.tar.zst",
      },
      expectedReferrer: null,
      expectedFilename: null,
    },
  ];

  for (const fixture of browserFixtures) {
    it(`${fixture.name} sends once and cancels only after the accepted ACK`, async () => {
      const events: string[] = [];
      const nativeResponse = deferred<unknown>();
      const sent: unknown[] = [];
      const downloads = downloadsFixture(events);
      const adapter = new FirefoxLinkInterception({
        downloads: downloads.api,
        health: { state: "connected" },
        messenger: {
          send(message) {
            events.push("native:send");
            sent.push(message);
            return nativeResponse.promise;
          },
        },
        createContext: () => context(),
      });

      const result = adapter.intercept(fixture.item);
      await Promise.resolve();
      expect(events).toEqual(["native:send"]);
      expect(sent).toEqual([
        expect.objectContaining({
          extension_id: firefoxExtensionId,
          payload: {
            type: "add_download",
            url: fixture.item.url,
            referrer: fixture.expectedReferrer,
            suggested_filename: fixture.expectedFilename,
          },
        }),
      ]);

      nativeResponse.resolve(response(context().requestId, "accepted"));
      await expect(result).resolves.toEqual({
        kind: "accepted",
        jobId: "018f47ce-8e51-7f31-a1ec-4a97f84a12ce",
      });
      expect(events).toEqual(["native:send", `cancel:${fixture.item.id}`]);

      await expect(adapter.intercept(fixture.item)).resolves.toEqual({
        kind: "ignored",
        reason: "duplicate",
      });
      expect(sent).toHaveLength(1);
    });
  }

  it("registers one Firefox download listener and removes that exact listener", () => {
    const events: string[] = [];
    const downloads = downloadsFixture(events);
    const adapter = new FirefoxLinkInterception({
      downloads: downloads.api,
      health: { state: "disconnected" },
      messenger: { send: async () => null },
      createContext: () => context(),
    });

    adapter.install();
    adapter.install();
    expect(downloads.listener()).toBeTypeOf("function");
    adapter.uninstall();
    adapter.uninstall();
    expect(downloads.listener()).toBeUndefined();
    expect(events).toEqual(["listener:add", "listener:remove"]);
  });

  it("leaves the original Firefox download running when offline or rejected", async () => {
    const source = "https://files.example.test/original.iso?token=signed";
    const scenarios = [
      {
        name: "offline",
        health: { state: "disconnected" as const },
        send: async () => {
          throw new Error("offline must not send");
        },
        reason: "native_unavailable",
      },
      {
        name: "rejected",
        health: { state: "connected" as const },
        send: async () => response(context().requestId, "rejected"),
        reason: "native_rejected",
      },
    ] as const;

    for (const [index, scenario] of scenarios.entries()) {
      const events: string[] = [];
      const fallbacks: unknown[] = [];
      const downloads = downloadsFixture(events);
      const adapter = new FirefoxLinkInterception({
        downloads: downloads.api,
        health: scenario.health,
        messenger: { send: scenario.send },
        createContext: () => context(),
        onFallback(details) {
          fallbacks.push(details);
        },
      });
      const item = { id: 70 + index, url: source };

      await expect(adapter.intercept(item)).resolves.toEqual({
        kind: "fallback",
        reason: scenario.reason,
      });
      await expect(adapter.intercept(item)).resolves.toEqual({
        kind: "ignored",
        reason: "duplicate",
      });
      expect(events).not.toContain(`cancel:${item.id}`);
      expect(fallbacks).toEqual([
        { downloadId: item.id, url: source, reason: scenario.reason },
      ]);
    }
  });

  it("times out once without cancelling the Firefox transfer", async () => {
    vi.useFakeTimers();
    const events: string[] = [];
    const fallbacks: unknown[] = [];
    try {
      const downloads = downloadsFixture(events);
      const adapter = new FirefoxLinkInterception({
        downloads: downloads.api,
        health: { state: "connected" },
        messenger: { send: () => new Promise(() => undefined) },
        createContext: () => context(),
        handoffTimeoutMs: 250,
        onFallback(details) {
          fallbacks.push(details);
        },
      });
      const item = {
        id: 90,
        url: "https://files.example.test/slow-download.bin",
      };

      const result = adapter.intercept(item);
      await vi.advanceTimersByTimeAsync(250);
      await expect(result).resolves.toEqual({
        kind: "fallback",
        reason: "native_timeout",
      });
      expect(events).not.toContain("cancel:90");
      expect(fallbacks).toEqual([
        {
          downloadId: 90,
          url: item.url,
          reason: "native_timeout",
        },
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("ignores unsafe schemes before native messaging or browser cancellation", async () => {
    let sends = 0;
    const events: string[] = [];
    const fallbacks: unknown[] = [];
    const downloads = downloadsFixture(events);
    const adapter = new FirefoxLinkInterception({
      downloads: downloads.api,
      health: { state: "connected" },
      messenger: {
        async send() {
          sends += 1;
          return null;
        },
      },
      createContext: () => context(),
      onFallback(details) {
        fallbacks.push(details);
      },
    });

    await expect(
      adapter.intercept({ id: 100, url: "file:///C:/private.txt" }),
    ).resolves.toEqual({ kind: "ignored", reason: "ineligible" });
    expect({ sends, events, fallbacks }).toEqual({
      sends: 0,
      events: [],
      fallbacks: [],
    });
  });
});
