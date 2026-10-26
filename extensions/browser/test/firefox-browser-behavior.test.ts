import { describe, expect, it, vi } from "vitest";

import {
  DEFAULT_CAPTURE_MODIFIERS,
  FIREFOX_ALL_LINKS_MENU_ID,
  FIREFOX_SELECTED_LINKS_MENU_ID,
  FirefoxBrowserBehavior,
  FirefoxLinkInterception,
  SINGLE_LINK_CONTEXT_MENU_ID,
  type AllLinksDocumentSource,
  type FirefoxContextMenusApi,
  type FirefoxDownloadItem,
  type FirefoxDownloadsApi,
  type FirefoxMenuClick,
  type FirefoxMenuTab,
  type LinkElementSource,
  type SelectedLinksDocumentSource,
  type SelectedLinksRangeSource,
} from "../src/index.js";

const extensionId = "download-manager@musharaf-hossain-cs.github.io";
const requestId = "018f47ce-8e51-7f31-a1ec-4a97f84a12cd";

function linksDocument(
  hrefs: readonly (string | null)[],
  baseURI = "https://www.example.test/releases/index.html",
): AllLinksDocumentSource {
  return {
    baseURI,
    querySelectorAll() {
      return hrefs.map<LinkElementSource>((href) => ({
        getAttribute: () => href,
      }));
    },
  };
}

function selectionRange(
  hrefs: readonly (string | null)[],
): SelectedLinksRangeSource {
  return {
    cloneContents() {
      return {
        querySelectorAll() {
          return hrefs.map<LinkElementSource>((href) => ({
            getAttribute: () => href,
          }));
        },
      };
    },
  };
}

function selectedDocument(
  ranges: readonly SelectedLinksRangeSource[] | null,
): SelectedLinksDocumentSource {
  return {
    baseURI: "https://www.example.test/releases/index.html",
    getSelection() {
      if (ranges === null) return null;
      return {
        rangeCount: ranges.length,
        getRangeAt(index) {
          const range = ranges[index];
          if (range === undefined) throw new RangeError("missing range");
          return range;
        },
      };
    },
  };
}

function downloadsFixture() {
  let listener: ((item: FirefoxDownloadItem) => void) | undefined;
  const cancelled: number[] = [];
  const api: FirefoxDownloadsApi = {
    onCreated: {
      addListener(next) {
        listener = next;
      },
      removeListener(next) {
        if (listener === next) listener = undefined;
      },
    },
    async cancel(downloadId) {
      cancelled.push(downloadId);
    },
  };
  return {
    api,
    cancelled,
    emit(item: FirefoxDownloadItem) {
      listener?.(item);
    },
    installed: () => listener !== undefined,
  };
}

function menuFixture() {
  const created: unknown[] = [];
  let listener:
    ((click: FirefoxMenuClick, tab: FirefoxMenuTab) => void) | undefined;
  const api: FirefoxContextMenusApi = {
    create(details) {
      created.push(details);
    },
    onClicked: {
      addListener(next) {
        listener = next;
      },
      removeListener(next) {
        if (listener === next) listener = undefined;
      },
    },
  };
  return { api, created, installed: () => listener !== undefined };
}

function behaviorFixture(
  modifiers = new Map<number, Partial<Record<"altKey" | "ctrlKey", boolean>>>(),
) {
  const downloads = downloadsFixture();
  const menus = menuFixture();
  const sent: {
    readonly payload?: { readonly type?: string };
    readonly request_id?: string;
  }[] = [];
  const dispatched: unknown[] = [];
  const interception = new FirefoxLinkInterception({
    downloads: downloads.api,
    health: { state: "connected" },
    messenger: {
      async send(message) {
        sent.push(message);
        return {
          version: 1,
          request_id: message.request_id,
          payload: {
            type: "accepted",
            job_ids: [`job-${String(sent.length)}`],
          },
        };
      },
    },
    createContext: () => ({
      requestId,
      nonce: "abcdefghijklmnop",
      extensionId,
    }),
  });
  const behavior = new FirefoxBrowserBehavior({
    contextMenus: menus.api,
    interception,
    pageBridge: {
      async allLinks() {
        return linksDocument([]);
      },
      async selectedLinks() {
        return selectedDocument(null);
      },
    },
    dispatch(request) {
      dispatched.push(request);
    },
    captureRules: {
      extensions: ["zip"],
      mimeTypes: ["application/pdf"],
      minimumSizeBytes: 1_024,
    },
    siteRules: {
      globalPolicy: "deny",
      sites: [{ hostPattern: "*.example.test", policy: "allow" }],
    },
    modifiers: DEFAULT_CAPTURE_MODIFIERS,
    captureModifiers(item) {
      const state = modifiers.get(item.id) ?? {};
      return {
        altKey: state.altKey ?? false,
        ctrlKey: state.ctrlKey ?? false,
        metaKey: false,
        shiftKey: false,
      };
    },
  });
  return { behavior, downloads, menus, sent, dispatched };
}

describe("Firefox shared browser behavior", () => {
  it("filters the installed interception path through site, type, force, and bypass rules", async () => {
    const modifiers = new Map([
      [3, { ctrlKey: true }],
      [4, { altKey: true }],
      [5, { altKey: true, ctrlKey: true }],
      [6, { ctrlKey: true }],
    ]);
    const fixture = behaviorFixture(modifiers);
    fixture.behavior.install();

    const items: readonly FirefoxDownloadItem[] = [
      {
        id: 1,
        url: "https://blocked.test/archive.zip",
        filename: "archive.zip",
        mime: "application/zip",
        totalBytes: 4_096,
      },
      {
        id: 2,
        url: "https://cdn.example.test/archive.zip",
        filename: "archive.zip",
        mime: "application/zip",
        totalBytes: 4_096,
      },
      {
        id: 3,
        url: "https://blocked.test/readme.txt",
        filename: "readme.txt",
        mime: "text/plain",
        totalBytes: 12,
      },
      {
        id: 4,
        url: "https://cdn.example.test/archive.zip",
        filename: "archive.zip",
        mime: "application/zip",
        totalBytes: 4_096,
      },
      {
        id: 5,
        url: "https://cdn.example.test/archive.zip",
        filename: "archive.zip",
        mime: "application/zip",
        totalBytes: 4_096,
      },
      {
        id: 6,
        url: "file:///C:/private/archive.zip",
        filename: "archive.zip",
        mime: "application/zip",
        totalBytes: 4_096,
      },
      {
        id: 7,
        url: "https://cdn.example.test/readme.txt",
        filename: "readme.txt",
        mime: "text/plain",
        totalBytes: 4_096,
      },
    ];
    for (const item of items) fixture.downloads.emit(item);

    await vi.waitFor(() => expect(fixture.sent).toHaveLength(2));
    expect(fixture.sent.map(({ payload }) => payload?.type)).toEqual([
      "add_download",
      "add_download",
    ]);
    expect(fixture.downloads.cancelled).toEqual([2, 3]);

    fixture.behavior.uninstall();
    expect(fixture.downloads.installed()).toBe(false);
    expect(fixture.menus.installed()).toBe(false);
  });

  const browserFixtures: readonly {
    readonly name: string;
    readonly tab: { readonly id: number };
    readonly omitOptionalClickFields: boolean;
  }[] = [
    { name: "stable", tab: { id: 17 }, omitOptionalClickFields: false },
    { name: "ESR", tab: { id: 18 }, omitOptionalClickFields: true },
  ];

  for (const browser of browserFixtures) {
    it(`${browser.name} installs the shared menus and dispatches normalized single/all/selected requests`, async () => {
      const allHrefs = Array.from(
        { length: 130 },
        (_, index) => `/all/file-${String(index)}.zip#fragment`,
      );
      allHrefs.splice(1, 0, "/all/file-0.zip#duplicate", "javascript:alert(1)");
      const selected = selectedDocument([
        selectionRange(["../chosen.zip#one", "mailto:help@example.test"]),
        selectionRange(["../chosen.zip#two", "//cdn.example.test/tool.exe"]),
      ]);

      const requests: unknown[] = [];
      const menus = menuFixture();
      const downloads = downloadsFixture();
      const interception = new FirefoxLinkInterception({
        downloads: downloads.api,
        health: { state: "disconnected" },
        messenger: { send: async () => null },
        createContext: () => ({
          requestId,
          nonce: "abcdefghijklmnop",
          extensionId,
        }),
      });
      const behavior = new FirefoxBrowserBehavior({
        contextMenus: menus.api,
        interception,
        pageBridge: {
          async allLinks(tabId) {
            expect(tabId).toBe(browser.tab.id);
            return linksDocument(allHrefs);
          },
          async selectedLinks(tabId) {
            expect(tabId).toBe(browser.tab.id);
            return selected;
          },
        },
        dispatch(request) {
          requests.push(request);
        },
        captureRules: {
          extensions: ["zip"],
          mimeTypes: [],
          minimumSizeBytes: 0,
        },
        siteRules: { globalPolicy: "allow", sites: [] },
        modifiers: DEFAULT_CAPTURE_MODIFIERS,
      });
      behavior.install();

      expect(menus.created).toEqual([
        {
          id: SINGLE_LINK_CONTEXT_MENU_ID,
          title: "Download with Download Manager",
          contexts: ["link"],
        },
        {
          id: FIREFOX_ALL_LINKS_MENU_ID,
          title: "Download all links with Download Manager",
          contexts: ["page"],
        },
        {
          id: FIREFOX_SELECTED_LINKS_MENU_ID,
          title: "Download selected links with Download Manager",
          contexts: ["selection"],
        },
      ]);

      await behavior.handleMenuClick(
        {
          menuItemId: SINGLE_LINK_CONTEXT_MENU_ID,
          linkUrl: "../single.zip#section",
          pageUrl: "https://www.example.test/releases/index.html",
          ...(browser.omitOptionalClickFields
            ? {}
            : { altKey: false, ctrlKey: false }),
        },
        browser.tab,
      );
      await behavior.handleMenuClick(
        { menuItemId: FIREFOX_ALL_LINKS_MENU_ID },
        browser.tab,
      );
      await behavior.handleMenuClick(
        { menuItemId: FIREFOX_SELECTED_LINKS_MENU_ID },
        browser.tab,
      );

      expect(requests[0]).toEqual({
        type: "add_download",
        url: "https://www.example.test/single.zip#section",
        referrer: "https://www.example.test/releases/index.html",
        suggested_filename: null,
      });
      expect(requests[1]).toMatchObject({ type: "add_batch" });
      expect((requests[1] as { items: readonly unknown[] }).items).toHaveLength(
        128,
      );
      expect(requests[2]).toEqual({
        type: "add_batch",
        items: [
          {
            url: "https://www.example.test/chosen.zip",
            referrer: "https://www.example.test/releases/index.html",
            suggested_filename: null,
          },
          {
            url: "https://cdn.example.test/tool.exe",
            referrer: "https://www.example.test/releases/index.html",
            suggested_filename: null,
          },
        ],
      });
    });
  }

  it("ignores empty, unsafe, unknown, and tabless menu requests", async () => {
    const fixture = behaviorFixture();
    await fixture.behavior.handleMenuClick(
      {
        menuItemId: SINGLE_LINK_CONTEXT_MENU_ID,
        linkUrl: "file:///C:/private.txt",
      },
      { id: 1 },
    );
    await fixture.behavior.handleMenuClick(
      { menuItemId: FIREFOX_SELECTED_LINKS_MENU_ID },
      { id: 1 },
    );
    await fixture.behavior.handleMenuClick(
      { menuItemId: FIREFOX_ALL_LINKS_MENU_ID },
      {},
    );
    await fixture.behavior.handleMenuClick(
      { menuItemId: "unknown" },
      { id: 1 },
    );
    expect(fixture.dispatched).toEqual([]);
  });
});
