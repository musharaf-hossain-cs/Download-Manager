import type { NativeConnectionHealth } from "./connection-health.js";
import {
  LinkHandoffController,
  type BrowserDownloadCandidate,
  type BrowserFallbackReason,
  type HandoffRequestContext,
  type LinkHandoffResult,
  type NativeDownloadMessenger,
} from "./link-handoff.js";

export interface FirefoxDownloadItem {
  readonly id: number;
  readonly url: string;
  readonly filename?: string;
  readonly referrer?: string;
}

export interface FirefoxDownloadsApi {
  readonly onCreated: {
    addListener(listener: (item: FirefoxDownloadItem) => void): void;
    removeListener(listener: (item: FirefoxDownloadItem) => void): void;
  };
  cancel(downloadId: number): Promise<void>;
}

export interface FirefoxFallbackDetails {
  readonly downloadId: number;
  readonly url: string;
  readonly reason: BrowserFallbackReason;
}

export interface FirefoxLinkInterceptionOptions {
  readonly downloads: FirefoxDownloadsApi;
  readonly health: Pick<NativeConnectionHealth, "state">;
  readonly messenger: NativeDownloadMessenger;
  readonly createContext: (
    candidate: BrowserDownloadCandidate,
  ) => HandoffRequestContext;
  readonly handoffTimeoutMs?: number;
  readonly onFallback?: (
    details: FirefoxFallbackDetails,
  ) => void | Promise<void>;
  readonly onError?: (error: unknown) => void;
}

/** Adapts Firefox download events to the shared acknowledged handoff contract. */
export class FirefoxLinkInterception {
  readonly #controller: LinkHandoffController;
  readonly #downloads: FirefoxDownloadsApi;
  readonly #onError: (error: unknown) => void;
  #installed = false;

  readonly #createdListener = (item: FirefoxDownloadItem): void => {
    void this.intercept(item).catch(this.#onError);
  };

  public constructor(options: FirefoxLinkInterceptionOptions) {
    this.#downloads = options.downloads;
    this.#onError = options.onError ?? (() => undefined);
    this.#controller = new LinkHandoffController(
      options.health,
      options.messenger,
      {
        cancel: (downloadId) => options.downloads.cancel(downloadId),
        fallback: async (details) => {
          // Firefox has already started this download. Leaving it untouched is
          // the fallback; starting another download would duplicate it.
          await options.onFallback?.(details);
        },
      },
      options.createContext,
      options.handoffTimeoutMs,
    );
  }

  public install(): void {
    if (this.#installed) return;
    this.#downloads.onCreated.addListener(this.#createdListener);
    this.#installed = true;
  }

  public uninstall(): void {
    if (!this.#installed) return;
    this.#downloads.onCreated.removeListener(this.#createdListener);
    this.#installed = false;
  }

  public intercept(item: FirefoxDownloadItem): Promise<LinkHandoffResult> {
    return this.#controller.handoff({
      downloadId: item.id,
      url: item.url,
      referrer: item.referrer ?? null,
      suggestedFilename: suggestedFilename(item.filename),
    });
  }
}

function suggestedFilename(filename: string | undefined): string | null {
  if (filename === undefined || filename.length === 0) return null;
  const component = filename.split(/[\\/]/u).at(-1);
  return component === undefined || component.length === 0 ? null : component;
}
