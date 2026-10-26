import { describe, expect, it } from "vitest";

import manifestJson from "../manifest.json" with { type: "json" };
import {
  installSingleLinkContextMenu,
  SINGLE_LINK_CONTEXT_MENU_ID,
  type SingleLinkMenuClick,
} from "../src/single-link-context-menu.js";

interface ChromiumManifest {
  readonly action?: { readonly default_title?: string };
  readonly background?: { readonly service_worker?: string };
  readonly content_scripts?: readonly unknown[];
  readonly content_security_policy?: { readonly extension_pages?: string };
  readonly description?: string;
  readonly host_permissions?: readonly string[];
  readonly manifest_version?: number;
  readonly name?: string;
  readonly permissions?: readonly string[];
  readonly version?: string;
  readonly web_accessible_resources?: readonly unknown[];
}

const manifest: ChromiumManifest = manifestJson;

describe("Chromium extension manifest", () => {
  it("is a clean-installable Manifest V3 shell", () => {
    expect(manifest).toMatchObject({
      manifest_version: 3,
      name: "Download Manager",
      version: "0.1.0",
      action: { default_title: "Download Manager" },
    });
    expect(manifest.description).toBeTruthy();
    expect(manifest.background).toBeUndefined();
  });

  it("requests only native handoff permissions and no site access", () => {
    expect(manifest.permissions).toEqual([
      "nativeMessaging",
      "downloads",
      "contextMenus",
    ]);
    expect(manifest.host_permissions).toBeUndefined();
    expect(manifest.content_scripts).toBeUndefined();
    expect(manifest.web_accessible_resources).toBeUndefined();
  });

  it("allows executable code only from the packaged extension", () => {
    const policy = manifest.content_security_policy?.extension_pages;

    expect(policy).toBe("script-src 'self'; object-src 'self'");
    expect(JSON.stringify(manifest)).not.toMatch(/https?:\/\//u);
  });
});

describe("single-link context menu", () => {
  function fixture() {
    let listener:
      ((click: SingleLinkMenuClick) => Promise<void> | void) | null = null;
    const created: unknown[] = [];
    const dispatched: unknown[] = [];
    installSingleLinkContextMenu(
      {
        create(details) {
          created.push(details);
        },
        onClicked(next) {
          listener = next;
        },
      },
      (item) => {
        dispatched.push(item);
      },
    );
    return {
      created,
      dispatched,
      click: async (info: SingleLinkMenuClick) => {
        await listener?.(info);
      },
    };
  }

  it("registers a link-only Download with Download Manager command", () => {
    const { created } = fixture();
    expect(created).toEqual([
      {
        id: SINGLE_LINK_CONTEXT_MENU_ID,
        title: "Download with Download Manager",
        contexts: ["link"],
      },
    ]);
  });

  it("dispatches direct and page-relative links as normalized items", async () => {
    const { click, dispatched } = fixture();
    await click({
      menuItemId: SINGLE_LINK_CONTEXT_MENU_ID,
      linkUrl: "https://cdn.example.test/files/direct.zip#part",
      pageUrl: "https://www.example.test/releases/index.html",
    });
    await click({
      menuItemId: SINGLE_LINK_CONTEXT_MENU_ID,
      linkUrl: "../files/relative.zip",
      pageUrl: "https://www.example.test/releases/current/",
    });

    expect(dispatched).toEqual([
      {
        url: "https://cdn.example.test/files/direct.zip#part",
        referrer: "https://www.example.test/releases/index.html",
        suggested_filename: null,
      },
      {
        url: "https://www.example.test/releases/files/relative.zip",
        referrer: "https://www.example.test/releases/current/",
        suggested_filename: null,
      },
    ]);
  });

  it("ignores other commands and unsafe or unresolved link targets", async () => {
    const { click, dispatched } = fixture();
    await click({
      menuItemId: "another-command",
      linkUrl: "https://files.example.test/ignored.zip",
    });
    await click({
      menuItemId: SINGLE_LINK_CONTEXT_MENU_ID,
      linkUrl: "javascript:alert(1)",
      pageUrl: "https://www.example.test/",
    });
    await click({
      menuItemId: SINGLE_LINK_CONTEXT_MENU_ID,
      linkUrl: "/missing-base.zip",
    });

    expect(dispatched).toEqual([]);
  });
});
