export interface ComponentIdentity {
  readonly name: string;
  readonly version: string;
}

export const desktopIdentity: ComponentIdentity = Object.freeze({
  name: "download-manager-desktop",
  version: "0.1.0",
});

export * from "./protocol.js";
export * from "./desktop-shell.js";
export * from "./download-list.js";
export * from "./engine-client.js";
export * from "./progress.js";
export * from "./virtual-list.js";
