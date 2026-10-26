import type { ExtensionDownloadItem } from "./protocol.js";

export const SINGLE_LINK_CONTEXT_MENU_ID = "download-with-download-manager";

export interface SingleLinkMenuClick {
  readonly menuItemId: string | number;
  readonly linkUrl?: string;
  readonly pageUrl?: string;
}

export interface SingleLinkContextMenuBrowser {
  create(details: {
    readonly id: typeof SINGLE_LINK_CONTEXT_MENU_ID;
    readonly title: "Download with Download Manager";
    readonly contexts: readonly ["link"];
  }): void;
  onClicked(
    listener: (click: SingleLinkMenuClick) => Promise<void> | void,
  ): void;
}

export type SingleLinkDispatcher = (
  item: ExtensionDownloadItem,
) => Promise<void> | void;

/** Registers the link-only command and routes valid HTTP(S) targets once. */
export function installSingleLinkContextMenu(
  browser: SingleLinkContextMenuBrowser,
  dispatch: SingleLinkDispatcher,
): void {
  browser.create({
    id: SINGLE_LINK_CONTEXT_MENU_ID,
    title: "Download with Download Manager",
    contexts: ["link"],
  });
  browser.onClicked(async (click) => {
    const item = normalizeSingleLinkClick(click);
    if (item !== null) await dispatch(item);
  });
}

export function normalizeSingleLinkClick(
  click: SingleLinkMenuClick,
): ExtensionDownloadItem | null {
  if (
    click.menuItemId !== SINGLE_LINK_CONTEXT_MENU_ID ||
    click.linkUrl === undefined
  ) {
    return null;
  }

  const page = parseHttpUrl(click.pageUrl);
  const link = parseHttpUrl(click.linkUrl, page ?? undefined);
  if (link === null || link.length > 8_192) return null;

  return {
    url: link,
    referrer: page,
    suggested_filename: null,
  };
}

function parseHttpUrl(value: string | undefined, base?: string): string | null {
  if (value === undefined || value.length > 8_192) return null;
  try {
    const url = new URL(value, base);
    if (
      (url.protocol !== "http:" && url.protocol !== "https:") ||
      url.hostname.length === 0
    ) {
      return null;
    }
    return url.toString();
  } catch {
    return null;
  }
}
