import { describe, expect, it } from "vitest";

import manifestJson from "../manifest.json" with { type: "json" };

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

  it("requests only the native bridge permission and no site access", () => {
    expect(manifest.permissions).toEqual(["nativeMessaging"]);
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
