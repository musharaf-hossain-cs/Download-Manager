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
