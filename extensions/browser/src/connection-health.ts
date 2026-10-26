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
  openDiagnostics(diagnostic: NativeConnectionDiagnostic): void;
}

export interface HealthRequestContext {
  readonly requestId: string;
  readonly nonce: string;
  readonly extensionId: string;
}

export type NativeConnectionDiagnosticCode =
  | "native_host_unavailable"
  | "invalid_health_response"
  | "protocol_version_mismatch"
  | "host_version_mismatch";

export interface NativeConnectionDiagnostic {
  readonly code: NativeConnectionDiagnosticCode;
  readonly expectedProtocolVersion: number;
  readonly actualProtocolVersion: number | null;
  readonly expectedHostVersion: string;
  readonly actualHostVersion: string | null;
  readonly recommendedAction: string;
}

/** Owns the native health handshake and its non-color badge state. */
export class NativeConnectionHealth {
  #state: NativeConnectionState = "disconnected";
  #requestId: string | null = null;
  #diagnostic: NativeConnectionDiagnostic;

  public constructor(
    private readonly bridge: NativeHealthBridge,
    private readonly expectedHostVersion: string,
  ) {
    this.#diagnostic = diagnostic(
      "native_host_unavailable",
      expectedHostVersion,
    );
    this.#render("disconnected");
  }

  public get state(): NativeConnectionState {
    return this.#state;
  }

  public get diagnostic(): NativeConnectionDiagnostic {
    return { ...this.#diagnostic };
  }

  /** Opens the desktop self-check with the extension's latest bounded evidence. */
  public openDiagnostics(): void {
    this.bridge.openDiagnostics(this.diagnostic);
  }

  public connect(context: HealthRequestContext): void {
    this.#requestId = context.requestId;
    this.#render("connecting");
    let port: NativeHealthPort;
    try {
      port = this.bridge.connectNative(NATIVE_HOST_NAME);
    } catch {
      this.#diagnostic = diagnostic(
        "native_host_unavailable",
        this.expectedHostVersion,
      );
      this.#render("disconnected");
      return;
    }
    port.onMessage((message) => this.#receive(message));
    port.onDisconnect(() => {
      this.#diagnostic = diagnostic(
        "native_host_unavailable",
        this.expectedHostVersion,
      );
      this.#render("disconnected");
    });
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
      this.#diagnostic = diagnostic(
        "invalid_health_response",
        this.expectedHostVersion,
      );
      this.#render("disconnected");
      return;
    }
    if (value.version !== EXTENSION_PROTOCOL_VERSION) {
      this.#diagnostic = diagnostic(
        "protocol_version_mismatch",
        this.expectedHostVersion,
        value.version,
        value.payload.host_version,
      );
      this.#render("version_mismatch");
      return;
    }
    if (value.payload.host_version !== this.expectedHostVersion) {
      this.#diagnostic = diagnostic(
        "host_version_mismatch",
        this.expectedHostVersion,
        value.version,
        value.payload.host_version,
      );
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

function diagnostic(
  code: NativeConnectionDiagnosticCode,
  expectedHostVersion: string,
  actualProtocolVersion: number | null = null,
  actualHostVersion: string | null = null,
): NativeConnectionDiagnostic {
  const recommendedAction =
    code === "host_version_mismatch"
      ? "Open diagnostics and update or reinstall Download Manager."
      : code === "protocol_version_mismatch"
        ? "Open diagnostics and update the application and extension together."
        : code === "invalid_health_response"
          ? "Open diagnostics to verify the registered native host."
          : "Open diagnostics to repair native-host registration.";
  return {
    code,
    expectedProtocolVersion: EXTENSION_PROTOCOL_VERSION,
    actualProtocolVersion,
    expectedHostVersion,
    actualHostVersion,
    recommendedAction,
  };
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
