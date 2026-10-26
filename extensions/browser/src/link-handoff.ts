import type { NativeConnectionHealth } from "./connection-health.js";
import {
  EXTENSION_PROTOCOL_VERSION,
  encodeExtensionRequest,
  type ExtensionRequestEnvelope,
} from "./protocol.js";

export interface BrowserDownloadCandidate {
  readonly downloadId: number;
  readonly url: string;
  readonly referrer: string | null;
  readonly suggestedFilename: string | null;
}

export interface HandoffRequestContext {
  readonly requestId: string;
  readonly nonce: string;
  readonly extensionId: string;
}

export interface NativeDownloadMessenger {
  send(message: ExtensionRequestEnvelope): Promise<unknown>;
}

export interface BrowserDownloadControl {
  cancel(downloadId: number): Promise<void>;
}

export type LinkHandoffResult =
  | { readonly kind: "accepted"; readonly jobId: string }
  | {
      readonly kind: "ignored";
      readonly reason: "ineligible" | "duplicate";
    }
  | {
      readonly kind: "rejected";
      readonly reason:
        "native_unavailable" | "native_rejected" | "invalid_response";
    };

/** Sends one eligible browser download and cancels it only after native ACK. */
export class LinkHandoffController {
  readonly #handledDownloads = new Set<number>();

  public constructor(
    private readonly health: Pick<NativeConnectionHealth, "state">,
    private readonly messenger: NativeDownloadMessenger,
    private readonly browser: BrowserDownloadControl,
    private readonly createContext: (
      candidate: BrowserDownloadCandidate,
    ) => HandoffRequestContext,
  ) {}

  public async handoff(
    candidate: BrowserDownloadCandidate,
  ): Promise<LinkHandoffResult> {
    const normalized = normalizeCandidate(candidate);
    if (normalized === null) return { kind: "ignored", reason: "ineligible" };
    if (this.#handledDownloads.has(candidate.downloadId)) {
      return { kind: "ignored", reason: "duplicate" };
    }
    this.#handledDownloads.add(candidate.downloadId);

    if (this.health.state !== "connected") {
      return { kind: "rejected", reason: "native_unavailable" };
    }
    const context = this.createContext(normalized);
    const payload = {
      type: "add_download" as const,
      url: normalized.url,
      referrer: normalized.referrer,
      suggested_filename: normalized.suggestedFilename,
    };
    encodeExtensionRequest({ ...context, payload });
    const request: ExtensionRequestEnvelope = {
      version: EXTENSION_PROTOCOL_VERSION,
      request_id: context.requestId,
      nonce: context.nonce,
      extension_id: context.extensionId,
      payload,
    };

    let response: unknown;
    try {
      response = await this.messenger.send(request);
    } catch {
      return { kind: "rejected", reason: "native_unavailable" };
    }
    if (isRejectedResponse(response, context.requestId)) {
      return { kind: "rejected", reason: "native_rejected" };
    }
    const jobId = acceptedJobId(response, context.requestId);
    if (jobId === null) {
      return { kind: "rejected", reason: "invalid_response" };
    }

    await this.browser.cancel(candidate.downloadId);
    return { kind: "accepted", jobId };
  }
}

function normalizeCandidate(
  candidate: BrowserDownloadCandidate,
): BrowserDownloadCandidate | null {
  if (!Number.isSafeInteger(candidate.downloadId) || candidate.downloadId < 0) {
    return null;
  }
  try {
    const url = new URL(candidate.url);
    if (!matchesHttp(url) || candidate.url.length > 8_192) return null;
    let referrer: string | null = null;
    if (candidate.referrer !== null) {
      const parsedReferrer = new URL(candidate.referrer);
      if (!matchesHttp(parsedReferrer) || candidate.referrer.length > 8_192) {
        return null;
      }
      referrer = parsedReferrer.toString();
    }
    return { ...candidate, url: url.toString(), referrer };
  } catch {
    return null;
  }
}

function matchesHttp(url: URL): boolean {
  return (
    (url.protocol === "http:" || url.protocol === "https:") &&
    url.hostname.length > 0
  );
}

function acceptedJobId(value: unknown, requestId: string): string | null {
  if (!matchesEnvelope(value, requestId)) return null;
  const payload = value.payload;
  if (
    payload.type !== "accepted" ||
    !Array.isArray(payload.job_ids) ||
    payload.job_ids.length !== 1 ||
    typeof payload.job_ids[0] !== "string" ||
    payload.job_ids[0].length === 0
  ) {
    return null;
  }
  return payload.job_ids[0];
}

function isRejectedResponse(value: unknown, requestId: string): boolean {
  return matchesEnvelope(value, requestId) && value.payload.type === "rejected";
}

function matchesEnvelope(
  value: unknown,
  requestId: string,
): value is {
  version: number;
  request_id: string;
  payload: Record<string, unknown>;
} {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return (
    candidate.version === EXTENSION_PROTOCOL_VERSION &&
    candidate.request_id === requestId &&
    typeof candidate.payload === "object" &&
    candidate.payload !== null
  );
}
