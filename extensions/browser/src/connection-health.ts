import {
  EXTENSION_PROTOCOL_VERSION,
  type ExtensionRequestEnvelope,
  type ExtensionResponseEnvelope,
} from "./protocol.js";

export const NATIVE_HOST_NAME =
  "io.github.musharaf_hossain_cs.download_manager";

export type NativeConnectionState =
  "connecting" | "connected" | "disconnected" | "version_mismatch";

export interface NativeHealthPort {
  postMessage(message: ExtensionRequestEnvelope): void;
  onMessage(listener: (message: unknown) => void): void;
  onDisconnect(listener: () => void): void;
}

export interface NativeHealthBridge {
  connectNative(hostName: string): NativeHealthPort;
  setBadge(presentation: {
    readonly text: string;
    readonly color: string;
    readonly title: string;
  }): void;
}

export interface HealthRequestContext {
  readonly requestId: string;
  readonly nonce: string;
  readonly extensionId: string;
}

/** Owns the native health handshake and its non-color badge state. */
export class NativeConnectionHealth {
  #state: NativeConnectionState = "disconnected";
  #requestId: string | null = null;

  public constructor(
    private readonly bridge: NativeHealthBridge,
    private readonly expectedHostVersion: string,
  ) {
    this.#render("disconnected");
  }

  public get state(): NativeConnectionState {
    return this.#state;
  }

  public connect(context: HealthRequestContext): void {
    this.#requestId = context.requestId;
    this.#render("connecting");
    let port: NativeHealthPort;
    try {
      port = this.bridge.connectNative(NATIVE_HOST_NAME);
    } catch {
      this.#render("disconnected");
      return;
    }
    port.onMessage((message) => this.#receive(message));
    port.onDisconnect(() => this.#render("disconnected"));
    port.postMessage({
      version: EXTENSION_PROTOCOL_VERSION,
      request_id: context.requestId,
      nonce: context.nonce,
      extension_id: context.extensionId,
      payload: { type: "health" },
    });
  }

  #receive(value: unknown): void {
    if (!isHealthResponse(value) || value.request_id !== this.#requestId) {
      this.#render("disconnected");
      return;
    }
    if (
      value.version !== EXTENSION_PROTOCOL_VERSION ||
      value.payload.host_version !== this.expectedHostVersion
    ) {
      this.#render("version_mismatch");
      return;
    }
    this.#render("connected");
  }

  #render(state: NativeConnectionState): void {
    this.#state = state;
    this.bridge.setBadge(BADGES[state]);
  }
}

const BADGES = {
  connecting: {
    text: "…",
    color: "#5f6368",
    title: "Download Manager: connecting",
  },
  connected: {
    text: "✓",
    color: "#137333",
    title: "Download Manager: connected",
  },
  disconnected: {
    text: "!",
    color: "#b3261e",
    title: "Download Manager: disconnected",
  },
  version_mismatch: {
    text: "↕",
    color: "#b06000",
    title: "Download Manager: version mismatch",
  },
} as const satisfies Record<NativeConnectionState, object>;

function isHealthResponse(
  value: unknown,
): value is ExtensionResponseEnvelope & {
  readonly payload: { readonly type: "healthy"; readonly host_version: string };
} {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Partial<ExtensionResponseEnvelope>;
  return (
    typeof candidate.version === "number" &&
    typeof candidate.request_id === "string" &&
    typeof candidate.payload === "object" &&
    candidate.payload !== null &&
    candidate.payload.type === "healthy" &&
    typeof candidate.payload.host_version === "string"
  );
}
