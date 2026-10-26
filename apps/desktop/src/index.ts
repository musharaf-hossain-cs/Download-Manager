export interface ComponentIdentity {
  readonly name: string;
  readonly version: string;
}

export const desktopIdentity: ComponentIdentity = Object.freeze({
  name: "download-manager-desktop",
  version: "0.1.0",
});

export * from "./protocol.js";
export * from "./add-url-dialog.js";
export * from "./appearance.js";
export * from "./categories.js";
export * from "./clipboard.js";
export * from "./desktop-shell.js";
export * from "./desktop-actions.js";
export * from "./drag-drop.js";
export * from "./download-list.js";
export * from "./engine-client.js";
export * from "./history.js";
export * from "./localization.js";
export * from "./notifications.js";
export * from "./progress.js";
export * from "./segment-detail.js";
export * from "./settings.js";
export * from "./tray.js";
export * from "./virtual-list.js";
