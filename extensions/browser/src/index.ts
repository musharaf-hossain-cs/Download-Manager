export type BrowserFamily = "chromium" | "firefox";

export interface ExtensionIdentity {
  readonly name: string;
  readonly supportedBrowsers: readonly BrowserFamily[];
  readonly version: string;
}

export const extensionIdentity: ExtensionIdentity = Object.freeze({
  name: "download-manager-browser-extension",
  supportedBrowsers: Object.freeze(["chromium", "firefox"] as const),
  version: "0.1.0",
});

export * from "./protocol.js";
export * from "./connection-health.js";
export * from "./link-handoff.js";
export * from "./capture-rules.js";
export * from "./site-rules.js";
export * from "./modifier-keys.js";
export * from "./single-link-context-menu.js";
export * from "./all-links.js";
