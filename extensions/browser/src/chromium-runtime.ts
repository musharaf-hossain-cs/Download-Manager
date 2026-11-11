import { CHROMIUM_EXTENSION_ID } from "./chromium-identity.js";
import { NATIVE_HOST_NAME } from "./connection-health.js";
import {
  LinkHandoffController,
  type BrowserDownloadCandidate,
  type HandoffRequestContext,
} from "./link-handoff.js";
import { normalizeSingleLinkClick } from "./single-link-context-menu.js";
import {
  EXTENSION_PROTOCOL_VERSION,
  type ExtensionDownloadItem,
  type ExtensionRequestEnvelope,
} from "./protocol.js";

export const CHROMIUM_FORCE_MENU_ID = "download-with-download-manager";
export const CHROMIUM_BYPASS_MENU_ID = "download-with-browser";
export const CHROMIUM_CAPTURE_ENABLED_KEY = "capture_enabled";

export interface ChromiumDownloadItem {
  readonly id: number;
  readonly url: string;
  readonly referrer?: string;
  readonly filename?: string;
}

export interface ChromiumMenuClick {
  readonly menuItemId: string | number;
  readonly linkUrl?: string;
  readonly pageUrl?: string;
}

interface ChromiumEvent<Listener extends (...arguments_: never[]) => unknown> {
  addListener(listener: Listener): void;
}

export interface ChromiumRuntimeApi {
  readonly action: {
    readonly onClicked: ChromiumEvent<() => void>;
    setBadgeBackgroundColor(details: { readonly color: string }): Promise<void>;
    setBadgeText(details: { readonly text: string }): Promise<void>;
    setTitle(details: { readonly title: string }): Promise<void>;
  };
  readonly contextMenus: {
    readonly onClicked: ChromiumEvent<(click: ChromiumMenuClick) => void>;
    create(details: {
      readonly id: string;
      readonly title: string;
      readonly contexts: readonly ["link"];
    }): void;
    removeAll(): Promise<void>;
  };
  readonly downloads: {
    readonly onCreated: ChromiumEvent<(item: ChromiumDownloadItem) => void>;
    cancel(downloadId: number): Promise<void>;
    download(details: {
      readonly url: string;
      readonly saveAs?: boolean;
    }): Promise<number>;
  };
  readonly runtime: {
    readonly id: string;
    readonly lastError?: { readonly message?: string };
    readonly onInstalled: ChromiumEvent<() => void>;
    sendNativeMessage(
      hostName: string,
      message: ExtensionRequestEnvelope,
      callback: (response: unknown) => void,
    ): void;
  };
  readonly storage: {
    readonly local: {
      get(key: string): Promise<Record<string, unknown>>;
      set(values: Record<string, unknown>): Promise<void>;
    };
  };
}

export type ChromiumRuntimeErrorHandler = (error: unknown) => void;

/**
 * Wires the permission-minimal Chromium MV3 worker to the shared handoff core.
 * Ordinary downloads remain browser-owned until the native app accepts one
 * durable job; an unavailable or rejecting app therefore needs no restart.
 */
export class ChromiumCaptureRuntime {
  readonly #bypassUrls = new Map<string, number>();
  readonly #controller: LinkHandoffController;
  readonly #ready: Promise<void>;
  #enabled = true;

  public constructor(
    private readonly api: ChromiumRuntimeApi,
    private readonly onError: ChromiumRuntimeErrorHandler = () => undefined,
    private readonly createContext: (
      candidate?: BrowserDownloadCandidate,
    ) => HandoffRequestContext = () =>
      createChromiumRequestContext(api.runtime.id),
  ) {
    this.#controller = new LinkHandoffController(
      { state: "connected" },
      { send: (message) => this.#sendNative(message) },
      {
        cancel: (downloadId) => this.api.downloads.cancel(downloadId),
        fallback: async () => undefined,
      },
      (candidate) => this.createContext(candidate),
    );
    this.#ready = this.#initialize();
    this.#installListeners();
  }

  public get enabled(): boolean {
    return this.#enabled;
  }

  public whenReady(): Promise<void> {
    return this.#ready;
  }

  #installListeners(): void {
    this.api.runtime.onInstalled.addListener(() => {
      void this.#rebuildMenus().catch(this.onError);
    });
    this.api.action.onClicked.addListener(() => {
      void this.#afterReady(() => this.#toggleEnabled());
    });
    this.api.contextMenus.onClicked.addListener((click) => {
      void this.#afterReady(() => this.#handleMenuClick(click));
    });
    this.api.downloads.onCreated.addListener((item) => {
      void this.#afterReady(() => this.#handleCreatedDownload(item));
    });
  }

  async #initialize(): Promise<void> {
    if (this.api.runtime.id !== CHROMIUM_EXTENSION_ID) {
      throw new Error(
        `unexpected Chromium extension ID: ${this.api.runtime.id}`,
      );
    }
    const stored = await this.api.storage.local.get(
      CHROMIUM_CAPTURE_ENABLED_KEY,
    );
    const enabled = stored[CHROMIUM_CAPTURE_ENABLED_KEY];
    this.#enabled = typeof enabled === "boolean" ? enabled : true;
    await this.#renderEnabled();
  }

  async #afterReady(operation: () => Promise<void>): Promise<void> {
    try {
      await this.#ready;
      await operation();
    } catch (error) {
      this.onError(error);
    }
  }

  async #rebuildMenus(): Promise<void> {
    await this.api.contextMenus.removeAll();
    this.api.contextMenus.create({
      id: CHROMIUM_FORCE_MENU_ID,
      title: "Download with Download Manager",
      contexts: ["link"],
    });
    this.api.contextMenus.create({
      id: CHROMIUM_BYPASS_MENU_ID,
      title: "Download in browser",
      contexts: ["link"],
    });
  }

  async #toggleEnabled(): Promise<void> {
    this.#enabled = !this.#enabled;
    await this.api.storage.local.set({
      [CHROMIUM_CAPTURE_ENABLED_KEY]: this.#enabled,
    });
    await this.#renderEnabled();
  }

  async #renderEnabled(): Promise<void> {
    await Promise.all([
      this.api.action.setBadgeBackgroundColor({ color: "#5f6368" }),
      this.api.action.setBadgeText({ text: this.#enabled ? "" : "OFF" }),
      this.api.action.setTitle({
        title: this.#enabled
          ? "Download Manager capture is enabled"
          : "Download Manager capture is disabled",
      }),
    ]);
  }

  async #handleCreatedDownload(item: ChromiumDownloadItem): Promise<void> {
    if (this.#consumeBypass(item.url) || !this.#enabled) return;
    await this.#controller.handoff({
      downloadId: item.id,
      url: item.url,
      referrer: item.referrer?.length ? item.referrer : null,
      suggestedFilename: filenameOnly(item.filename),
    });
  }

  async #handleMenuClick(click: ChromiumMenuClick): Promise<void> {
    const normalized = normalizeSingleLinkClick({
      ...click,
      menuItemId: CHROMIUM_FORCE_MENU_ID,
    });
    if (normalized === null) return;
    if (click.menuItemId === CHROMIUM_BYPASS_MENU_ID) {
      await this.#downloadInBrowser(normalized.url, true);
      return;
    }
    if (click.menuItemId !== CHROMIUM_FORCE_MENU_ID) return;
    await this.#forceHandoff(normalized);
  }

  async #forceHandoff(item: ExtensionDownloadItem): Promise<void> {
    const context = this.createContext();
    const request: ExtensionRequestEnvelope = {
      version: EXTENSION_PROTOCOL_VERSION,
      request_id: context.requestId,
      nonce: context.nonce,
      extension_id: context.extensionId,
      payload: { type: "add_download", ...item },
    };
    try {
      const response = await this.#sendNative(request);
      if (acceptedJobId(response, context.requestId) !== null) return;
    } catch {
      // Explicit force still preserves the user's download when the app is off.
    }
    await this.#downloadInBrowser(item.url, false);
  }

  async #downloadInBrowser(url: string, saveAs: boolean): Promise<void> {
    this.#markBypass(url);
    try {
      await this.api.downloads.download({ url, saveAs });
    } catch (error) {
      this.#consumeBypass(url);
      throw error;
    }
  }

  #markBypass(url: string): void {
    this.#bypassUrls.set(url, (this.#bypassUrls.get(url) ?? 0) + 1);
  }

  #consumeBypass(url: string): boolean {
    const count = this.#bypassUrls.get(url) ?? 0;
    if (count === 0) return false;
    if (count === 1) this.#bypassUrls.delete(url);
    else this.#bypassUrls.set(url, count - 1);
    return true;
  }

  #sendNative(message: ExtensionRequestEnvelope): Promise<unknown> {
    return new Promise((resolve, reject) => {
      this.api.runtime.sendNativeMessage(
        NATIVE_HOST_NAME,
        message,
        (response) => {
          const lastError = this.api.runtime.lastError;
          if (lastError !== undefined) {
            reject(new Error(lastError.message ?? "native messaging failed"));
          } else {
            resolve(response);
          }
        },
      );
    });
  }
}

export function createChromiumRequestContext(
  extensionId: string,
): HandoffRequestContext {
  const random = new Uint8Array(18);
  crypto.getRandomValues(random);
  const nonce = bytesToBase64Url(random);
  return {
    requestId: crypto.randomUUID(),
    nonce,
    extensionId,
  };
}

function filenameOnly(value: string | undefined): string | null {
  if (value === undefined || value.length === 0) return null;
  const name = value.split(/[\\/]/u).at(-1);
  return name?.length ? name : null;
}

function bytesToBase64Url(value: Uint8Array): string {
  let binary = "";
  for (const byte of value) binary += String.fromCharCode(byte);
  return btoa(binary)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/u, "");
}

function acceptedJobId(value: unknown, requestId: string): string | null {
  if (typeof value !== "object" || value === null) return null;
  const envelope = value as Record<string, unknown>;
  if (
    envelope.version !== EXTENSION_PROTOCOL_VERSION ||
    envelope.request_id !== requestId ||
    typeof envelope.payload !== "object" ||
    envelope.payload === null
  ) {
    return null;
  }
  const payload = envelope.payload as Record<string, unknown>;
  const ids = payload.job_ids;
  return payload.type === "accepted" &&
    Array.isArray(ids) &&
    ids.length === 1 &&
    typeof ids[0] === "string" &&
    ids[0].length > 0
    ? ids[0]
    : null;
}
