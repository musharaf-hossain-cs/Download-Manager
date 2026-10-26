import { describe, expect, it } from "vitest";
import { URL } from "node:url";

import {
  DEFAULT_CAPTURE_MODIFIERS,
  FIREFOX_ALL_LINKS_MENU_ID,
  FIREFOX_SELECTED_LINKS_MENU_ID,
  FirefoxBrowserBehavior,
  FirefoxLinkInterception,
  LinkHandoffController,
  SINGLE_LINK_CONTEXT_MENU_ID,
  collectAllLinks,
  collectSelectedLinks,
  evaluateCaptureRules,
  evaluateSiteCapture,
  normalizeSingleLinkClick,
  resolveCaptureModifier,
} from "../src/index.js";
import {
  BROWSER_REGRESSION_MATRIX,
  BROWSER_REGRESSION_SCENARIOS,
  createBrowserRegressionReport,
  serializeBrowserRegressionReport,
} from "../../../scripts/browser-regression.mjs";

const requestId = "018f47ce-8e51-7f31-a1ec-4a97f84a12cd";
const jobId = "018f47ce-8e51-7f31-a1ec-4a97f84a12ce";
const extensionId = "abcdefghijklmnopabcdefghijklmnop";
const firefoxExtensionId = "download-manager@musharaf-hossain-cs.github.io";
const none = Object.freeze({
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
});

function context(variant) {
  return {
    requestId,
    nonce: "abcdefghijklmnop",
    extensionId:
      variant.adapter === "firefox" ? firefoxExtensionId : extensionId,
  };
}

function accepted(request) {
  return {
    version: 1,
    request_id: request.request_id,
    payload: { type: "accepted", job_ids: [jobId] },
  };
}

async function acceptedHandoff(variant) {
  const events = [];
  let sends = 0;
  const candidate = {
    downloadId: 42,
    url: "https://files.example.test/archive.zip",
    referrer: "https://www.example.test/releases",
    suggestedFilename: "archive.zip",
  };

  if (variant.adapter === "firefox") {
    const adapter = new FirefoxLinkInterception({
      downloads: {
        onCreated: { addListener() {}, removeListener() {} },
        async cancel(id) {
          events.push(`cancel:${String(id)}`);
        },
      },
      health: { state: "connected" },
      messenger: {
        async send(request) {
          sends += 1;
          events.push("native:send");
          return accepted(request);
        },
      },
      createContext: () => context(variant),
    });
    const item = {
      id: candidate.downloadId,
      url: candidate.url,
      referrer: candidate.referrer,
      filename: candidate.suggestedFilename,
    };
    const result = await adapter.intercept(item);
    const duplicate = await adapter.intercept(item);
    return { result, duplicate, events, sends };
  }

  const controller = new LinkHandoffController(
    { state: "connected" },
    {
      async send(request) {
        sends += 1;
        events.push("native:send");
        return accepted(request);
      },
    },
    {
      async cancel(id) {
        events.push(`cancel:${String(id)}`);
      },
      async fallback() {
        throw new Error("accepted handoff must not fallback");
      },
    },
    () => context(variant),
  );
  const result = await controller.handoff(candidate);
  const duplicate = await controller.handoff(candidate);
  return { result, duplicate, events, sends };
}

async function offlineFallback(variant) {
  const fallbacks = [];
  const cancelled = [];
  const originalUrl = "https://files.example.test/archive.zip?token=signed";
  if (variant.adapter === "firefox") {
    const adapter = new FirefoxLinkInterception({
      downloads: {
        onCreated: { addListener() {}, removeListener() {} },
        async cancel(id) {
          cancelled.push(id);
        },
      },
      health: { state: "disconnected" },
      messenger: { send: async () => null },
      createContext: () => context(variant),
      onFallback(details) {
        fallbacks.push(details.url);
      },
    });
    const result = await adapter.intercept({ id: 7, url: originalUrl });
    return { result, fallbacks, cancelled, originalUrl };
  }

  const controller = new LinkHandoffController(
    { state: "disconnected" },
    { send: async () => null },
    {
      async cancel(id) {
        cancelled.push(id);
      },
      async fallback(details) {
        fallbacks.push(details.url);
      },
    },
    () => context(variant),
  );
  const result = await controller.handoff({
    downloadId: 7,
    url: originalUrl,
    referrer: null,
    suggestedFilename: null,
  });
  return { result, fallbacks, cancelled, originalUrl };
}

function captureDecision(variant, candidate, modifiers) {
  const captureRules = {
    extensions: ["zip"],
    mimeTypes: ["application/pdf"],
    minimumSizeBytes: 1_024,
  };
  const siteRules = {
    globalPolicy: "deny",
    sites: [{ hostPattern: "*.example.test", policy: "allow" }],
  };
  if (variant.adapter === "firefox") {
    return firefoxBehavior({ captureRules, siteRules }).evaluateCapture({
      ...candidate,
      modifiers,
    }).capture;
  }
  if (!isHttpUrl(candidate.url)) return false;
  const modifier = resolveCaptureModifier(modifiers, DEFAULT_CAPTURE_MODIFIERS);
  if (modifier.action === "bypass") return false;
  if (modifier.action === "force") return true;
  return (
    evaluateSiteCapture(candidate.url, siteRules).capture &&
    evaluateCaptureRules(candidate, captureRules).capture
  );
}

function isHttpUrl(value) {
  try {
    const url = new URL(value);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      url.hostname.length > 0
    );
  } catch {
    return false;
  }
}

function firefoxBehavior({
  captureRules,
  siteRules,
  pageBridge,
  dispatch,
} = {}) {
  return new FirefoxBrowserBehavior({
    contextMenus: {
      create() {},
      onClicked: { addListener() {}, removeListener() {} },
    },
    interception: { install() {}, uninstall() {} },
    pageBridge: pageBridge ?? {
      async allLinks() {
        return linksDocument([]);
      },
      async selectedLinks() {
        return selectedDocument([]);
      },
    },
    dispatch: dispatch ?? (() => undefined),
    captureRules: captureRules ?? {
      extensions: ["zip"],
      mimeTypes: [],
      minimumSizeBytes: 0,
    },
    siteRules: siteRules ?? { globalPolicy: "allow", sites: [] },
    modifiers: DEFAULT_CAPTURE_MODIFIERS,
  });
}

function link(value) {
  return { getAttribute: () => value };
}

function linksDocument(values) {
  return {
    baseURI: "https://www.example.test/releases/index.html",
    querySelectorAll() {
      return values.map(link);
    },
  };
}

function selectedDocument(values) {
  return {
    baseURI: "https://www.example.test/releases/index.html",
    getSelection() {
      return {
        rangeCount: 1,
        getRangeAt() {
          return {
            cloneContents() {
              return { querySelectorAll: () => values.map(link) };
            },
          };
        },
      };
    },
  };
}

async function menuRequests(variant) {
  const all = linksDocument([
    "../one.zip#first",
    "../one.zip#duplicate",
    "javascript:alert(1)",
    "//cdn.example.test/two.exe",
  ]);
  const selected = selectedDocument([
    "../chosen.zip#one",
    "../chosen.zip#two",
    "mailto:help@example.test",
  ]);
  const singleClick = {
    menuItemId: SINGLE_LINK_CONTEXT_MENU_ID,
    linkUrl: "../single.zip",
    pageUrl: all.baseURI,
  };

  if (variant.adapter === "firefox") {
    const requests = [];
    const behavior = firefoxBehavior({
      pageBridge: {
        async allLinks() {
          return all;
        },
        async selectedLinks() {
          return selected;
        },
      },
      dispatch(request) {
        requests.push(request);
      },
    });
    await behavior.handleMenuClick(singleClick, { id: 9 });
    await behavior.handleMenuClick(
      { menuItemId: FIREFOX_ALL_LINKS_MENU_ID },
      { id: 9 },
    );
    await behavior.handleMenuClick(
      { menuItemId: FIREFOX_SELECTED_LINKS_MENU_ID },
      { id: 9 },
    );
    return requests;
  }

  const single = normalizeSingleLinkClick(singleClick);
  const allResult = await collectAllLinks(all);
  const selectedResult = await collectSelectedLinks(selected);
  return [
    { type: "add_download", ...single },
    { type: "add_batch", items: allResult.items },
    { type: "add_batch", items: selectedResult.items },
  ];
}

describe("cross-browser adapter fixture matrix", () => {
  for (const variant of BROWSER_REGRESSION_MATRIX) {
    const label = `${variant.browser}-${variant.channel}`;

    it(`${label} accepts once and cancels only after the native ACK`, async () => {
      const result = await acceptedHandoff(variant);
      expect(result).toEqual({
        result: { kind: "accepted", jobId },
        duplicate: { kind: "ignored", reason: "duplicate" },
        events: ["native:send", "cancel:42"],
        sends: 1,
      });
    });

    it(`${label} preserves the original URL when native is offline`, async () => {
      const result = await offlineFallback(variant);
      expect(result.result).toEqual({
        kind: "fallback",
        reason: "native_unavailable",
      });
      expect(result.fallbacks).toEqual([result.originalUrl]);
      expect(result.cancelled).toEqual([]);
    });

    it(`${label} shares site, type, force, and conflict-safe bypass rules`, () => {
      const allowed = {
        url: "https://cdn.example.test/archive.zip",
        suggestedFilename: "archive.zip",
        mimeType: "application/zip",
        sizeBytes: 4_096,
      };
      const denied = { ...allowed, url: "https://blocked.test/readme.txt" };
      expect(captureDecision(variant, allowed, none)).toBe(true);
      expect(captureDecision(variant, denied, none)).toBe(false);
      expect(captureDecision(variant, denied, { ...none, ctrlKey: true })).toBe(
        true,
      );
      expect(captureDecision(variant, allowed, { ...none, altKey: true })).toBe(
        false,
      );
      expect(
        captureDecision(variant, allowed, {
          ...none,
          altKey: true,
          ctrlKey: true,
        }),
      ).toBe(false);
    });

    it(`${label} shares normalized single/all/selected menu batches`, async () => {
      const requests = await menuRequests(variant);
      expect(requests).toEqual([
        {
          type: "add_download",
          url: "https://www.example.test/single.zip",
          referrer: "https://www.example.test/releases/index.html",
          suggested_filename: null,
        },
        {
          type: "add_batch",
          items: [
            {
              url: "https://www.example.test/one.zip",
              referrer: "https://www.example.test/releases/index.html",
              suggested_filename: null,
            },
            {
              url: "https://cdn.example.test/two.exe",
              referrer: "https://www.example.test/releases/index.html",
              suggested_filename: null,
            },
          ],
        },
        {
          type: "add_batch",
          items: [
            {
              url: "https://www.example.test/chosen.zip",
              referrer: "https://www.example.test/releases/index.html",
              suggested_filename: null,
            },
          ],
        },
      ]);
    });
  }

  it("serializes a stable, explicitly scoped result artifact", () => {
    expect(BROWSER_REGRESSION_SCENARIOS).toHaveLength(4);
    expect(serializeBrowserRegressionReport()).toBe(
      serializeBrowserRegressionReport(),
    );
    expect(createBrowserRegressionReport()).toMatchObject({
      schema_version: 1,
      scope: "browser_adapter_fixture",
      command: "pnpm test:browser-regression",
      browsers: [
        { browser: "chrome", channel: "stable", adapter: "chromium" },
        { browser: "edge", channel: "stable", adapter: "chromium" },
        { browser: "firefox", channel: "stable", adapter: "firefox" },
        { browser: "firefox", channel: "esr", adapter: "firefox" },
      ],
    });
  });
});
