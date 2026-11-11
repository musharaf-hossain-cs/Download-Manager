import { describe, expect, it, vi } from "vitest";

import {
  CHROMIUM_BYPASS_MENU_ID,
  CHROMIUM_CAPTURE_ENABLED_KEY,
  CHROMIUM_EXTENSION_ID,
  CHROMIUM_FORCE_MENU_ID,
  ChromiumCaptureRuntime,
  type ChromiumDownloadItem,
  type ChromiumMenuClick,
  type ChromiumRuntimeApi,
} from "../src/index.js";
import type { ExtensionRequestEnvelope } from "../src/protocol.js";

const requestContext = {
  requestId: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
  nonce: "abcdefghijklmnop",
  extensionId: CHROMIUM_EXTENSION_ID,
};

function fixture(options: { enabled?: boolean; offline?: boolean } = {}) {
  let actionListener: (() => void) | undefined;
  let installedListener: (() => void) | undefined;
  let menuListener: ((click: ChromiumMenuClick) => void) | undefined;
  let downloadListener: ((item: ChromiumDownloadItem) => void) | undefined;
  const storage: Record<string, unknown> = {};
  if (options.enabled !== undefined) {
    storage[CHROMIUM_CAPTURE_ENABLED_KEY] = options.enabled;
  }
  const nativeMessages: ExtensionRequestEnvelope[] = [];
  const cancelled: number[] = [];
  const browserDownloads: Array<{ url: string; saveAs?: boolean }> = [];
  const menus: unknown[] = [];
  const errors: unknown[] = [];
  const badges: string[] = [];
  const runtime: {
    id: string;
    lastError?: { message: string };
    onInstalled: { addListener(listener: () => void): void };
    sendNativeMessage(
      host: string,
      message: ExtensionRequestEnvelope,
      callback: (response: unknown) => void,
    ): void;
  } = {
    id: CHROMIUM_EXTENSION_ID,
    onInstalled: {
      addListener(listener) {
        installedListener = listener;
      },
    },
    sendNativeMessage(_host, message, callback) {
      nativeMessages.push(message);
      if (options.offline) {
        runtime.lastError = { message: "host unavailable" };
        callback(undefined);
        delete runtime.lastError;
      } else {
        callback({
          version: 1,
          request_id: message.request_id,
          payload: { type: "accepted", job_ids: ["job-1"] },
        });
      }
    },
  };
  const api: ChromiumRuntimeApi = {
    action: {
      onClicked: { addListener: (listener) => (actionListener = listener) },
      setBadgeBackgroundColor: async () => undefined,
      setBadgeText: async ({ text }) => {
        badges.push(text);
      },
      setTitle: async () => undefined,
    },
    contextMenus: {
      onClicked: { addListener: (listener) => (menuListener = listener) },
      create: (details) => menus.push(details),
      removeAll: async () => {
        menus.length = 0;
      },
    },
    downloads: {
      onCreated: { addListener: (listener) => (downloadListener = listener) },
      cancel: async (id) => {
        cancelled.push(id);
      },
      download: async (details) => {
        browserDownloads.push(details);
        return 900 + browserDownloads.length;
      },
    },
    runtime,
    storage: {
      local: {
        get: async () => ({ ...storage }),
        set: async (values) => {
          Object.assign(storage, values);
        },
      },
    },
  };
  const capture = new ChromiumCaptureRuntime(
    api,
    (error) => errors.push(error),
    () => requestContext,
  );
  return {
    capture,
    storage,
    nativeMessages,
    cancelled,
    browserDownloads,
    menus,
    errors,
    badges,
    action: () => actionListener?.(),
    install: () => installedListener?.(),
    click: (click: ChromiumMenuClick) => menuListener?.(click),
    created: (item: ChromiumDownloadItem) => downloadListener?.(item),
  };
}

async function settled(): Promise<void> {
  await vi.waitFor(() => undefined);
  await Promise.resolve();
  await Promise.resolve();
}

describe("Chromium MV3 runtime", () => {
  it("persists the enabled toggle and installs only link commands", async () => {
    const state = fixture({ enabled: false });
    await state.capture.whenReady();
    expect(state.capture.enabled).toBe(false);
    expect(state.badges.at(-1)).toBe("OFF");

    state.install();
    await settled();
    expect(state.menus).toEqual([
      {
        id: CHROMIUM_FORCE_MENU_ID,
        title: "Download with Download Manager",
        contexts: ["link"],
      },
      {
        id: CHROMIUM_BYPASS_MENU_ID,
        title: "Download in browser",
        contexts: ["link"],
      },
    ]);

    state.action();
    await vi.waitFor(() => expect(state.capture.enabled).toBe(true));
    expect(state.storage[CHROMIUM_CAPTURE_ENABLED_KEY]).toBe(true);
  });

  it("hands an ordinary fixture download to the app exactly once", async () => {
    const state = fixture();
    await state.capture.whenReady();
    const item = {
      id: 42,
      url: "https://files.example.test/archive.zip",
      referrer: "https://www.example.test/releases",
      filename: "C:\\Downloads\\archive.zip",
    };
    state.created(item);
    state.created(item);
    await vi.waitFor(() => expect(state.cancelled).toEqual([42]));
    expect(state.nativeMessages).toHaveLength(1);
    expect(state.nativeMessages[0]?.payload).toEqual({
      type: "add_download",
      url: item.url,
      referrer: item.referrer,
      suggested_filename: "archive.zip",
    });
  });

  it("leaves the original browser transfer untouched while the app is off", async () => {
    const state = fixture({ offline: true });
    await state.capture.whenReady();
    state.created({ id: 7, url: "https://files.example.test/offline.zip" });
    await vi.waitFor(() => expect(state.nativeMessages).toHaveLength(1));
    expect(state.cancelled).toEqual([]);
    expect(state.browserDownloads).toEqual([]);
    expect(state.errors).toEqual([]);
  });

  it("supports explicit bypass without recapturing the launched download", async () => {
    const state = fixture();
    await state.capture.whenReady();
    const url = "https://files.example.test/browser.zip";
    state.click({ menuItemId: CHROMIUM_BYPASS_MENU_ID, linkUrl: url });
    await vi.waitFor(() => expect(state.browserDownloads).toHaveLength(1));
    state.created({ id: 9, url });
    await settled();
    expect(state.browserDownloads).toEqual([{ url, saveAs: true }]);
    expect(state.nativeMessages).toEqual([]);
    expect(state.cancelled).toEqual([]);
  });

  it("forces a disabled link to native and falls back once when offline", async () => {
    const accepted = fixture({ enabled: false });
    await accepted.capture.whenReady();
    const url = "https://files.example.test/forced.zip";
    accepted.click({ menuItemId: CHROMIUM_FORCE_MENU_ID, linkUrl: url });
    await vi.waitFor(() => expect(accepted.nativeMessages).toHaveLength(1));
    expect(accepted.browserDownloads).toEqual([]);

    const offline = fixture({ enabled: false, offline: true });
    await offline.capture.whenReady();
    offline.click({ menuItemId: CHROMIUM_FORCE_MENU_ID, linkUrl: url });
    await vi.waitFor(() => expect(offline.browserDownloads).toHaveLength(1));
    expect(offline.browserDownloads).toEqual([{ url, saveAs: false }]);
    expect(offline.nativeMessages).toHaveLength(1);
  });
});
