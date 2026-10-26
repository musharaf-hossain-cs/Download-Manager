import {
  MAX_EXTENSION_BATCH_ITEMS,
  type ExtensionDownloadItem,
} from "./protocol.js";
import { normalizeHttpUrl } from "./http-url.js";

export const MAX_COLLECTED_LINKS = MAX_EXTENSION_BATCH_ITEMS;
export const DEFAULT_MAX_COLLECTED_LINKS = MAX_COLLECTED_LINKS;
export const DEFAULT_MAX_SCANNED_LINKS = 50_000;
export const MAX_SCANNED_LINKS = 100_000;
export const DEFAULT_LINK_SCAN_BATCH_SIZE = 256;

export interface LinkElementSource {
  getAttribute(name: "href"): string | null;
}

export interface AllLinksDocumentSource {
  readonly baseURI: string;
  querySelectorAll(
    selector: "a[href], area[href]",
  ): Iterable<LinkElementSource>;
}

export interface AllLinksScheduler {
  yield(): Promise<void>;
}

export interface CollectAllLinksOptions {
  readonly maxUniqueLinks?: number;
  readonly maxScannedLinks?: number;
  readonly batchSize?: number;
  readonly scheduler?: AllLinksScheduler;
}

export interface AllLinksCollection {
  readonly items: readonly ExtensionDownloadItem[];
  readonly scannedCount: number;
  readonly duplicateCount: number;
  readonly rejectedCount: number;
  readonly limitReached: "unique" | "scanned" | null;
}

/** Collects page links without monopolizing the content-script event loop. */
export async function collectAllLinks(
  document: AllLinksDocumentSource,
  options: CollectAllLinksOptions = {},
): Promise<AllLinksCollection> {
  const maxUniqueLinks = boundedInteger(
    "maxUniqueLinks",
    options.maxUniqueLinks ?? DEFAULT_MAX_COLLECTED_LINKS,
    MAX_COLLECTED_LINKS,
  );
  const maxScannedLinks = boundedInteger(
    "maxScannedLinks",
    options.maxScannedLinks ?? DEFAULT_MAX_SCANNED_LINKS,
    MAX_SCANNED_LINKS,
  );
  const batchSize = boundedInteger(
    "batchSize",
    options.batchSize ?? DEFAULT_LINK_SCAN_BATCH_SIZE,
    1_024,
  );
  const pageUrl = canonicalDownloadUrl(document.baseURI);
  if (pageUrl === null) {
    throw new TypeError("document base URI must be an absolute HTTP(S) URL");
  }

  const items: ExtensionDownloadItem[] = [];
  const seen = new Set<string>();
  let scannedCount = 0;
  let duplicateCount = 0;
  let rejectedCount = 0;
  let limitReached: AllLinksCollection["limitReached"] = null;
  const scheduler = options.scheduler ?? EVENT_LOOP_SCHEDULER;

  const source = document.querySelectorAll("a[href], area[href]");
  const elements = source[Symbol.iterator]();
  while (scannedCount < maxScannedLinks) {
    const next = elements.next();
    if (next.done) break;
    scannedCount += 1;
    const url = canonicalDownloadUrl(next.value.getAttribute("href"), pageUrl);
    if (url === null) {
      rejectedCount += 1;
    } else if (seen.has(url)) {
      duplicateCount += 1;
    } else {
      seen.add(url);
      items.push({ url, referrer: pageUrl, suggested_filename: null });
      if (items.length === maxUniqueLinks) {
        limitReached = "unique";
        break;
      }
    }

    if (scannedCount % batchSize === 0) await scheduler.yield();
  }
  if (limitReached === null && scannedCount === maxScannedLinks) {
    limitReached = "scanned";
  }

  return {
    items,
    scannedCount,
    duplicateCount,
    rejectedCount,
    limitReached,
  };
}

function canonicalDownloadUrl(
  value: string | null,
  base?: string,
): string | null {
  const normalized = normalizeHttpUrl(value ?? undefined, base);
  if (normalized === null || normalized.length > 8_192) return null;
  const url = new URL(normalized);
  url.hash = "";
  return url.toString();
}

function boundedInteger(name: string, value: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
    throw new RangeError(`${name} must be an integer between 1 and ${maximum}`);
  }
  return value;
}

const EVENT_LOOP_SCHEDULER: AllLinksScheduler = {
  yield: () => new Promise((resolve) => setTimeout(resolve, 0)),
};
