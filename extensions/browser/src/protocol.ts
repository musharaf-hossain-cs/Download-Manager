export const EXTENSION_PROTOCOL_VERSION = 1 as const;
export const MAX_EXTENSION_MESSAGE_BYTES = 65_536;
export const MAX_EXTENSION_BATCH_ITEMS = 128;

export interface ExtensionDownloadItem {
  readonly url: string;
  readonly referrer: string | null;
  readonly suggested_filename: string | null;
}

export type ExtensionRequest =
  | { readonly type: "health" }
  | ({ readonly type: "add_download" } & ExtensionDownloadItem)
  | {
      readonly type: "add_batch";
      readonly items: readonly ExtensionDownloadItem[];
    };

export interface ExtensionRequestEnvelope {
  readonly version: typeof EXTENSION_PROTOCOL_VERSION;
  readonly request_id: string;
  readonly nonce: string;
  readonly extension_id: string;
  readonly payload: ExtensionRequest;
}

export interface ExtensionRequestParameters {
  readonly requestId: string;
  readonly nonce: string;
  readonly extensionId: string;
  readonly payload: ExtensionRequest;
}

export type ExtensionResponse =
  | { readonly type: "healthy"; readonly host_version: string }
  | { readonly type: "accepted"; readonly job_ids: readonly string[] }
  | {
      readonly type: "rejected";
      readonly code: string;
      readonly message: string;
    };

export interface ExtensionResponseEnvelope {
  readonly version: typeof EXTENSION_PROTOCOL_VERSION;
  readonly request_id: string;
  readonly payload: ExtensionResponse;
}

export function encodeExtensionRequest(
  parameters: ExtensionRequestParameters,
): Uint8Array {
  validateNonce(parameters.nonce);
  if (
    parameters.extensionId.length === 0 ||
    parameters.extensionId.length > 256
  ) {
    throw new Error("invalid extension ID");
  }
  validatePayload(parameters.payload);

  const envelope: ExtensionRequestEnvelope = {
    version: EXTENSION_PROTOCOL_VERSION,
    request_id: parameters.requestId,
    nonce: parameters.nonce,
    extension_id: parameters.extensionId,
    payload: parameters.payload,
  };
  const encoded = new TextEncoder().encode(JSON.stringify(envelope));
  if (encoded.byteLength > MAX_EXTENSION_MESSAGE_BYTES) {
    throw new Error("extension message exceeds 65536 bytes");
  }
  return encoded;
}

function validateNonce(nonce: string): void {
  if (!/^[A-Za-z0-9_-]{16,128}$/u.test(nonce)) {
    throw new Error("invalid request nonce");
  }
}

function validatePayload(payload: ExtensionRequest): void {
  const items =
    payload.type === "add_batch"
      ? payload.items
      : payload.type === "add_download"
        ? [payload]
        : [];
  if (items.length > MAX_EXTENSION_BATCH_ITEMS) {
    throw new Error("extension batch exceeds 128 items");
  }
  for (const item of items) {
    validateHttpUrl(item.url);
    if (item.referrer !== null) {
      validateHttpUrl(item.referrer);
    }
    if (
      item.suggested_filename !== null &&
      (item.suggested_filename.length === 0 ||
        item.suggested_filename.length > 1_024 ||
        /[\0\r\n]/u.test(item.suggested_filename))
    ) {
      throw new Error("invalid suggested filename");
    }
  }
}

function validateHttpUrl(value: string): void {
  if (value.length > 8_192) {
    throw new Error("URL exceeds 8192 bytes");
  }
  const url = new URL(value);
  if ((url.protocol !== "http:" && url.protocol !== "https:") || !url.host) {
    throw new Error("only absolute HTTP(S) URLs are supported");
  }
}
