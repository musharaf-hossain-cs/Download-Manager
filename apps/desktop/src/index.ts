export interface ComponentIdentity {
  readonly name: string;
  readonly version: string;
}

export const desktopIdentity: ComponentIdentity = Object.freeze({
  name: "download-manager-desktop",
  version: "0.1.0",
});
