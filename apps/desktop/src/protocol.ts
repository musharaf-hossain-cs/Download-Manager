export const UI_PROTOCOL_VERSION = 1 as const;

export type DownloadState =
  | "created"
  | "probing"
  | "queued"
  | "connecting"
  | "downloading"
  | "pausing"
  | "paused"
  | "verifying"
  | "assembling"
  | "completed"
  | "retry_waiting"
  | "needs_authentication"
  | "needs_refresh"
  | "blocked"
  | "failed"
  | "cancelled"
  | "missing_local_file";

export type DownloadKind = "file" | "media";
export type DownloadPriority = "low" | "normal" | "high";
export type DuplicateAction =
  "focus_existing" | "add_duplicate" | "replace_existing";
export type JobAction = "start" | "pause" | "resume" | "stop";

/** Complete, non-secret job metadata returned by the engine. */
export interface DownloadJob {
  readonly id: string;
  readonly source_url: string;
  readonly final_url: string | null;
  readonly display_url: string;
  readonly filename: string;
  readonly destination: string;
  readonly state: DownloadState;
  readonly kind: DownloadKind;
  readonly total_bytes: number | null;
  readonly downloaded_bytes: number;
  readonly category: string | null;
  readonly queue: string | null;
  readonly priority: DownloadPriority;
  readonly source_browser: string | null;
  readonly source_profile: string | null;
  readonly retry_count: number;
  readonly error: string | null;
  readonly created_at: number;
  readonly updated_at: number;
  readonly completed_at: number | null;
}

export interface UrlProbe {
  readonly final_url: string;
  readonly filename: string;
  readonly destination: string;
  readonly total_bytes: number | null;
  readonly mime_type: string | null;
}

export type UiRequest =
  | { readonly type: "health" }
  | { readonly type: "list_jobs" }
  | {
      readonly type: "probe_url";
      readonly source_url: string;
      readonly destination: string;
    }
  | {
      readonly type: "add_job";
      readonly source_url: string;
      readonly final_url: string;
      readonly filename: string;
      readonly destination: string;
      readonly total_bytes: number | null;
      readonly start_now: boolean;
      readonly duplicate_action: DuplicateAction;
    }
  | {
      readonly type: "control_job";
      readonly job_id: string;
      readonly action: JobAction;
    };

export interface UiRequestEnvelope {
  readonly version: typeof UI_PROTOCOL_VERSION;
  readonly request_id: string;
  readonly payload: UiRequest;
}

export type UiResponse =
  | { readonly type: "health"; readonly engine_version: string }
  | { readonly type: "accepted"; readonly job_id: string }
  | { readonly type: "jobs"; readonly jobs: readonly DownloadJob[] }
  | ({ readonly type: "url_probe" } & UrlProbe)
  | {
      readonly type: "job_state";
      readonly job_id: string;
      readonly state: DownloadState;
      readonly updated_at: number;
    }
  | {
      readonly type: "rejected";
      readonly code: string;
      readonly message: string;
    };

export interface StateTransition {
  readonly job_id: string;
  readonly from: DownloadState;
  readonly to: DownloadState;
  readonly at: number;
  readonly reason: string | null;
}

export type DownloadEvent =
  | {
      readonly type: "state_changed";
      readonly transition: StateTransition;
    }
  | {
      readonly type: "progress";
      readonly job_id: string;
      readonly downloaded_bytes: number;
      readonly total_bytes: number | null;
      readonly at: number;
    }
  | {
      readonly type: "error";
      readonly job_id: string;
      readonly code: string;
      readonly message: string;
      readonly retryable: boolean;
      readonly at: number;
    };

export type UiMessage =
  | { readonly type: "response"; readonly response: UiResponse }
  | { readonly type: "event"; readonly event: DownloadEvent };

export interface UiMessageEnvelope {
  readonly version: typeof UI_PROTOCOL_VERSION;
  readonly request_id: string;
  readonly payload: UiMessage;
}

export function createUiRequest(
  requestId: string,
  payload: UiRequest,
): UiRequestEnvelope {
  return {
    version: UI_PROTOCOL_VERSION,
    request_id: requestId,
    payload,
  };
}

export class ProtocolViolationError extends Error {
  public constructor(message: string) {
    super(message);
    this.name = "ProtocolViolationError";
  }
}

const DOWNLOAD_STATES: ReadonlySet<string> = new Set<DownloadState>([
  "created",
  "probing",
  "queued",
  "connecting",
  "downloading",
  "pausing",
  "paused",
  "verifying",
  "assembling",
  "completed",
  "retry_waiting",
  "needs_authentication",
  "needs_refresh",
  "blocked",
  "failed",
  "cancelled",
  "missing_local_file",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(
  value: Record<string, unknown>,
  keys: readonly string[],
): boolean {
  const actual = Object.keys(value);
  return (
    actual.length === keys.length && actual.every((key) => keys.includes(key))
  );
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function isNullableNonNegativeNumber(value: unknown): value is number | null {
  return value === null || isNonNegativeNumber(value);
}

function isNonNegativeNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isDownloadJob(value: unknown): value is DownloadJob {
  if (!isRecord(value)) return false;
  const keys = [
    "id",
    "source_url",
    "final_url",
    "display_url",
    "filename",
    "destination",
    "state",
    "kind",
    "total_bytes",
    "downloaded_bytes",
    "category",
    "queue",
    "priority",
    "source_browser",
    "source_profile",
    "retry_count",
    "error",
    "created_at",
    "updated_at",
    "completed_at",
  ] as const;
  return (
    hasExactKeys(value, keys) &&
    typeof value.id === "string" &&
    typeof value.source_url === "string" &&
    isNullableString(value.final_url) &&
    typeof value.display_url === "string" &&
    typeof value.filename === "string" &&
    typeof value.destination === "string" &&
    typeof value.state === "string" &&
    DOWNLOAD_STATES.has(value.state) &&
    (value.kind === "file" || value.kind === "media") &&
    isNullableNonNegativeNumber(value.total_bytes) &&
    isNonNegativeNumber(value.downloaded_bytes) &&
    isNullableString(value.category) &&
    isNullableString(value.queue) &&
    (value.priority === "low" ||
      value.priority === "normal" ||
      value.priority === "high") &&
    isNullableString(value.source_browser) &&
    isNullableString(value.source_profile) &&
    isNonNegativeNumber(value.retry_count) &&
    isNullableString(value.error) &&
    isNonNegativeNumber(value.created_at) &&
    isNonNegativeNumber(value.updated_at) &&
    isNullableNonNegativeNumber(value.completed_at) &&
    (value.total_bytes === null || value.downloaded_bytes <= value.total_bytes)
  );
}

function parseResponse(value: unknown): UiResponse {
  if (!isRecord(value) || typeof value.type !== "string") {
    throw new ProtocolViolationError(
      "engine response is not an object with a type",
    );
  }
  switch (value.type) {
    case "health":
      if (
        hasExactKeys(value, ["type", "engine_version"]) &&
        typeof value.engine_version === "string"
      ) {
        return value as unknown as UiResponse;
      }
      break;
    case "accepted":
      if (
        hasExactKeys(value, ["type", "job_id"]) &&
        typeof value.job_id === "string"
      ) {
        return value as unknown as UiResponse;
      }
      break;
    case "jobs":
      if (
        hasExactKeys(value, ["type", "jobs"]) &&
        Array.isArray(value.jobs) &&
        value.jobs.every(isDownloadJob)
      ) {
        return value as unknown as UiResponse;
      }
      break;
    case "url_probe":
      if (
        hasExactKeys(value, [
          "type",
          "final_url",
          "filename",
          "destination",
          "total_bytes",
          "mime_type",
        ]) &&
        typeof value.final_url === "string" &&
        typeof value.filename === "string" &&
        typeof value.destination === "string" &&
        isNullableNonNegativeNumber(value.total_bytes) &&
        isNullableString(value.mime_type)
      ) {
        return value as unknown as UiResponse;
      }
      break;
    case "job_state":
      if (
        hasExactKeys(value, ["type", "job_id", "state", "updated_at"]) &&
        typeof value.job_id === "string" &&
        typeof value.state === "string" &&
        DOWNLOAD_STATES.has(value.state) &&
        isNonNegativeNumber(value.updated_at)
      ) {
        return value as unknown as UiResponse;
      }
      break;
    case "rejected":
      if (
        hasExactKeys(value, ["type", "code", "message"]) &&
        typeof value.code === "string" &&
        typeof value.message === "string"
      ) {
        return value as unknown as UiResponse;
      }
      break;
  }
  throw new ProtocolViolationError(`invalid ${value.type} engine response`);
}

function parseEvent(value: unknown): DownloadEvent {
  if (!isRecord(value) || typeof value.type !== "string") {
    throw new ProtocolViolationError(
      "engine event is not an object with a type",
    );
  }
  if (
    value.type === "progress" &&
    hasExactKeys(value, [
      "type",
      "job_id",
      "downloaded_bytes",
      "total_bytes",
      "at",
    ]) &&
    typeof value.job_id === "string" &&
    isNonNegativeNumber(value.downloaded_bytes) &&
    isNullableNonNegativeNumber(value.total_bytes) &&
    isNonNegativeNumber(value.at) &&
    (value.total_bytes === null || value.downloaded_bytes <= value.total_bytes)
  ) {
    return value as unknown as DownloadEvent;
  }
  if (
    value.type === "error" &&
    hasExactKeys(value, [
      "type",
      "job_id",
      "code",
      "message",
      "retryable",
      "at",
    ]) &&
    typeof value.job_id === "string" &&
    typeof value.code === "string" &&
    typeof value.message === "string" &&
    typeof value.retryable === "boolean" &&
    isNonNegativeNumber(value.at)
  ) {
    return value as unknown as DownloadEvent;
  }
  if (
    value.type === "state_changed" &&
    hasExactKeys(value, ["type", "transition"]) &&
    isRecord(value.transition) &&
    hasExactKeys(value.transition, ["job_id", "from", "to", "at", "reason"]) &&
    typeof value.transition.job_id === "string" &&
    typeof value.transition.from === "string" &&
    DOWNLOAD_STATES.has(value.transition.from) &&
    typeof value.transition.to === "string" &&
    DOWNLOAD_STATES.has(value.transition.to) &&
    isNonNegativeNumber(value.transition.at) &&
    isNullableString(value.transition.reason)
  ) {
    return value as unknown as DownloadEvent;
  }
  throw new ProtocolViolationError(`invalid ${value.type} engine event`);
}

/** Validates an untrusted engine message before it enters desktop state. */
export function decodeUiMessage(value: unknown): UiMessageEnvelope {
  if (
    !isRecord(value) ||
    !hasExactKeys(value, ["version", "request_id", "payload"]) ||
    value.version !== UI_PROTOCOL_VERSION ||
    typeof value.request_id !== "string" ||
    !isRecord(value.payload) ||
    typeof value.payload.type !== "string"
  ) {
    throw new ProtocolViolationError("invalid engine message envelope");
  }
  if (
    value.payload.type === "response" &&
    hasExactKeys(value.payload, ["type", "response"])
  ) {
    return {
      version: UI_PROTOCOL_VERSION,
      request_id: value.request_id,
      payload: {
        type: "response",
        response: parseResponse(value.payload.response),
      },
    };
  }
  if (
    value.payload.type === "event" &&
    hasExactKeys(value.payload, ["type", "event"])
  ) {
    return {
      version: UI_PROTOCOL_VERSION,
      request_id: value.request_id,
      payload: { type: "event", event: parseEvent(value.payload.event) },
    };
  }
  throw new ProtocolViolationError("invalid engine message payload");
}
