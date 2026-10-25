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

export type UiRequest =
  | { readonly type: "health" }
  | { readonly type: "list_jobs" }
  | {
      readonly type: "add_job";
      readonly source_url: string;
      readonly filename: string | null;
      readonly start_now: boolean;
    }
  | {
      readonly type: "transition_job";
      readonly job_id: string;
      readonly state: DownloadState;
    };

export interface UiRequestEnvelope {
  readonly version: typeof UI_PROTOCOL_VERSION;
  readonly request_id: string;
  readonly payload: UiRequest;
}

export type UiResponse =
  | { readonly type: "health"; readonly engine_version: string }
  | { readonly type: "accepted"; readonly job_id: string }
  | { readonly type: "jobs"; readonly job_ids: readonly string[] }
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
