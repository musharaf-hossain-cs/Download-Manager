import type { NativeConnectionHealth } from "./connection-health.js";
import {
  MediaCandidateModel,
  type MediaCandidate,
} from "./media-candidates.js";
import type {
  HandoffRequestContext,
  NativeDownloadMessenger,
} from "./link-handoff.js";
import {
  EXTENSION_PROTOCOL_VERSION,
  encodeExtensionRequest,
  type ExtensionRequestEnvelope,
} from "./protocol.js";

export interface ProgressiveMediaVariant {
  readonly candidate: MediaCandidate;
  readonly referrer: string;
  readonly suggestedFilename?: string | null;
}

export type ProgressiveMediaHandoffResult =
  | { readonly kind: "accepted"; readonly jobId: string }
  | { readonly kind: "ignored"; readonly reason: "ineligible" | "duplicate" }
  | {
      readonly kind: "rejected";
      readonly reason:
        | "native_unavailable"
        | "native_rejected"
        | "invalid_response"
        | "native_timeout";
    };

/** Hands one validated progressive media variant to the native ACK contract. */
export class ProgressiveMediaHandoffController {
  readonly #handled = new Set<string>();

  public constructor(
    private readonly health: Pick<NativeConnectionHealth, "state">,
    private readonly messenger: NativeDownloadMessenger,
    private readonly createContext: (
      variant: ProgressiveMediaVariant,
    ) => HandoffRequestContext,
    private readonly handoffTimeoutMs = 3_000,
  ) {
    if (!Number.isFinite(handoffTimeoutMs) || handoffTimeoutMs <= 0) {
      throw new RangeError("handoff timeout must be positive");
    }
  }

  public async handoff(
    variant: ProgressiveMediaVariant,
  ): Promise<ProgressiveMediaHandoffResult> {
    const normalized = normalizeVariant(variant);
    if (normalized === null) return { kind: "ignored", reason: "ineligible" };
    const key = `${normalized.candidate.url}\n${normalized.referrer}`;
    if (this.#handled.has(key)) return { kind: "ignored", reason: "duplicate" };
    if (this.health.state !== "connected") {
      return { kind: "rejected", reason: "native_unavailable" };
    }
    this.#handled.add(key);

    let accepted = false;
    try {
      const context = this.createContext(normalized);
      const payload = {
        type: "add_download" as const,
        url: normalized.candidate.url,
        referrer: normalized.referrer,
        suggested_filename: normalized.suggestedFilename ?? null,
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
        response = await withTimeout(
          this.messenger.send(request),
          this.handoffTimeoutMs,
        );
      } catch {
        return { kind: "rejected", reason: "native_unavailable" };
      }
      if (response === NATIVE_TIMEOUT) {
        return { kind: "rejected", reason: "native_timeout" };
      }
      if (isRejectedResponse(response, context.requestId)) {
        return { kind: "rejected", reason: "native_rejected" };
      }
      const jobId = acceptedJobId(response, context.requestId);
      if (jobId === null) {
        return { kind: "rejected", reason: "invalid_response" };
      }
      accepted = true;
      return { kind: "accepted", jobId };
    } finally {
      if (!accepted) this.#handled.delete(key);
    }
  }
}

function normalizeVariant(
  variant: ProgressiveMediaVariant,
): ProgressiveMediaVariant | null {
  if (variant.candidate.kind !== "progressive") return null;
  try {
    const referrer = new URL(variant.referrer);
    if (
      (referrer.protocol !== "http:" && referrer.protocol !== "https:") ||
      !referrer.hostname ||
      variant.referrer.length > 8_192
    ) {
      return null;
    }
    referrer.hash = "";
    const model = new MediaCandidateModel(referrer.toString());
    const observation = model.observe({
      url: variant.candidate.url,
      mimeType: variant.candidate.mimeType,
      source: "media-element",
    });
    if (
      observation.status === "rejected" ||
      observation.candidate.kind !== "progressive"
    ) {
      return null;
    }
    const filename = variant.suggestedFilename ?? null;
    if (
      filename !== null &&
      (filename.length === 0 ||
        filename.length > 1_024 ||
        /[\0\r\n]/u.test(filename))
    ) {
      return null;
    }
    return {
      candidate: observation.candidate,
      referrer: referrer.toString(),
      suggestedFilename: filename,
    };
  } catch {
    return null;
  }
}

const NATIVE_TIMEOUT = Symbol("native timeout");

async function withTimeout(
  response: Promise<unknown>,
  timeoutMs: number,
): Promise<unknown | typeof NATIVE_TIMEOUT> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<typeof NATIVE_TIMEOUT>((resolve) => {
    timer = setTimeout(() => resolve(NATIVE_TIMEOUT), timeoutMs);
  });
  try {
    return await Promise.race([response, timeout]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

function acceptedJobId(value: unknown, requestId: string): string | null {
  if (!matchesEnvelope(value, requestId)) return null;
  const jobs = value.payload.job_ids;
  return value.payload.type === "accepted" &&
    Array.isArray(jobs) &&
    jobs.length === 1 &&
    typeof jobs[0] === "string" &&
    jobs[0].length > 0
    ? jobs[0]
    : null;
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
  const response = value as Record<string, unknown>;
  return (
    response.version === EXTENSION_PROTOCOL_VERSION &&
    response.request_id === requestId &&
    typeof response.payload === "object" &&
    response.payload !== null
  );
}
