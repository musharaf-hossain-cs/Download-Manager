import {
  FirefoxLinkInterception,
  type FirefoxDownloadItem,
} from "./firefox-link-interception.js";
import {
  collectAllLinks,
  type AllLinksDocumentSource,
  type CollectAllLinksOptions,
} from "./all-links.js";
import {
  evaluateCaptureRules,
  type CaptureRuleCandidate,
  type CaptureRuleConfig,
} from "./capture-rules.js";
import { normalizeHttpUrl } from "./http-url.js";
import {
  resolveCaptureModifier,
  type BrowserModifierState,
  type CaptureModifierConfig,
} from "./modifier-keys.js";
import type { ExtensionRequest } from "./protocol.js";
import {
  collectSelectedLinks,
  type SelectedLinksDocumentSource,
} from "./selected-links.js";
import { evaluateSiteCapture, type SiteCaptureConfig } from "./site-rules.js";
import {
  normalizeSingleLinkClick,
  SINGLE_LINK_CONTEXT_MENU_ID,
  type SingleLinkMenuClick,
} from "./single-link-context-menu.js";

export const FIREFOX_ALL_LINKS_MENU_ID =
  "download-all-links-with-download-manager";
export const FIREFOX_SELECTED_LINKS_MENU_ID =
  "download-selected-links-with-download-manager";

export interface FirefoxMenuClick extends SingleLinkMenuClick {
  readonly altKey?: boolean;
  readonly ctrlKey?: boolean;
  readonly metaKey?: boolean;
  readonly shiftKey?: boolean;
}

export interface FirefoxMenuTab {
  readonly id?: number;
}

export interface FirefoxContextMenusApi {
  create(details: {
    readonly id: string;
    readonly title: string;
    readonly contexts: readonly ("link" | "page" | "selection")[];
  }): void;
  readonly onClicked: {
    addListener(
      listener: (click: FirefoxMenuClick, tab: FirefoxMenuTab) => void,
    ): void;
    removeListener(
      listener: (click: FirefoxMenuClick, tab: FirefoxMenuTab) => void,
    ): void;
  };
}

/** Supplies page-owned DOM sources without granting the extension broad host access. */
export interface FirefoxActivePageBridge {
  allLinks(tabId: number): Promise<AllLinksDocumentSource>;
  selectedLinks(tabId: number): Promise<SelectedLinksDocumentSource>;
}

export type FirefoxRequestDispatcher = (
  request: ExtensionRequest,
) => Promise<void> | void;

export interface FirefoxBehaviorOptions {
  readonly contextMenus: FirefoxContextMenusApi;
  readonly interception: Pick<FirefoxLinkInterception, "install" | "uninstall">;
  readonly pageBridge: FirefoxActivePageBridge;
  readonly dispatch: FirefoxRequestDispatcher;
  readonly captureRules: CaptureRuleConfig;
  readonly siteRules: SiteCaptureConfig;
  readonly modifiers: CaptureModifierConfig;
  readonly captureModifiers?: (
    item: FirefoxDownloadItem,
  ) => BrowserModifierState;
  readonly collection?: CollectAllLinksOptions;
  readonly onError?: (error: unknown) => void;
}

export interface FirefoxCaptureCandidate extends CaptureRuleCandidate {
  readonly modifiers: BrowserModifierState;
}

export type FirefoxCaptureDecision =
  | {
      readonly capture: true;
      readonly reason: "force_modifier" | "rules_match";
    }
  | {
      readonly capture: false;
      readonly reason:
        | "invalid_url"
        | "bypass_modifier"
        | "modifier_conflict"
        | "site_rule"
        | "type_rule";
    };

/** Shares capture policy and link-menu behavior across Firefox stable and ESR. */
export class FirefoxBrowserBehavior {
  readonly #options: FirefoxBehaviorOptions;
  readonly #onError: (error: unknown) => void;
  #installed = false;

  readonly #menuListener = (
    click: FirefoxMenuClick,
    tab: FirefoxMenuTab,
  ): void => {
    void this.handleMenuClick(click, tab).catch(this.#onError);
  };

  public constructor(options: FirefoxBehaviorOptions) {
    this.#options = options;
    this.#onError = options.onError ?? (() => undefined);
  }

  public install(): void {
    if (this.#installed) return;
    this.#options.contextMenus.create({
      id: SINGLE_LINK_CONTEXT_MENU_ID,
      title: "Download with Download Manager",
      contexts: ["link"],
    });
    this.#options.contextMenus.create({
      id: FIREFOX_ALL_LINKS_MENU_ID,
      title: "Download all links with Download Manager",
      contexts: ["page"],
    });
    this.#options.contextMenus.create({
      id: FIREFOX_SELECTED_LINKS_MENU_ID,
      title: "Download selected links with Download Manager",
      contexts: ["selection"],
    });
    this.#options.contextMenus.onClicked.addListener(this.#menuListener);
    this.#options.interception.install((item) =>
      this.shouldInterceptDownload(item),
    );
    this.#installed = true;
  }

  public uninstall(): void {
    if (!this.#installed) return;
    this.#options.interception.uninstall();
    this.#options.contextMenus.onClicked.removeListener(this.#menuListener);
    this.#installed = false;
  }

  public evaluateCapture(
    candidate: FirefoxCaptureCandidate,
  ): FirefoxCaptureDecision {
    if (normalizeHttpUrl(candidate.url) === null) {
      return { capture: false, reason: "invalid_url" };
    }
    const modifier = resolveCaptureModifier(
      candidate.modifiers,
      this.#options.modifiers,
    );
    if (modifier.action === "bypass") {
      return {
        capture: false,
        reason:
          modifier.reason === "modifier_conflict"
            ? "modifier_conflict"
            : "bypass_modifier",
      };
    }
    if (modifier.action === "force") {
      return { capture: true, reason: "force_modifier" };
    }
    if (!evaluateSiteCapture(candidate.url, this.#options.siteRules).capture) {
      return { capture: false, reason: "site_rule" };
    }
    if (!evaluateCaptureRules(candidate, this.#options.captureRules).capture) {
      return { capture: false, reason: "type_rule" };
    }
    return { capture: true, reason: "rules_match" };
  }

  public shouldInterceptDownload(item: FirefoxDownloadItem): boolean {
    return this.evaluateCapture({
      url: item.url,
      suggestedFilename: item.filename ?? null,
      mimeType: item.mime ?? null,
      sizeBytes: item.totalBytes ?? null,
      modifiers: this.#options.captureModifiers?.(item) ?? NO_MODIFIERS,
    }).capture;
  }

  public async handleMenuClick(
    click: FirefoxMenuClick,
    tab: FirefoxMenuTab,
  ): Promise<void> {
    if (click.menuItemId === SINGLE_LINK_CONTEXT_MENU_ID) {
      const item = normalizeSingleLinkClick(click);
      if (item !== null) {
        await this.#options.dispatch({ type: "add_download", ...item });
      }
      return;
    }

    const tabId = tab.id;
    if (tabId === undefined || !Number.isSafeInteger(tabId) || tabId < 0)
      return;
    const collection =
      click.menuItemId === FIREFOX_ALL_LINKS_MENU_ID
        ? await collectAllLinks(
            await this.#options.pageBridge.allLinks(tabId),
            this.#options.collection,
          )
        : click.menuItemId === FIREFOX_SELECTED_LINKS_MENU_ID
          ? await collectSelectedLinks(
              await this.#options.pageBridge.selectedLinks(tabId),
              this.#options.collection,
            )
          : null;
    if (collection !== null && collection.items.length > 0) {
      await this.#options.dispatch({
        type: "add_batch",
        items: collection.items,
      });
    }
  }
}

const NO_MODIFIERS: BrowserModifierState = Object.freeze({
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
});
