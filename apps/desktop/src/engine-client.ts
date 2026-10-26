import {
  createUiRequest,
  decodeUiMessage,
  ProtocolViolationError,
  type DownloadEvent,
  type DownloadJob,
  type UiRequest,
  type UiRequestEnvelope,
  type UiResponse,
  type UrlProbe,
} from "./protocol.js";

export interface EngineTransport {
  send(message: UiRequestEnvelope): void | Promise<void>;
  subscribe(listener: (message: unknown) => void): () => void;
  close(): void | Promise<void>;
}

export interface EngineLauncher {
  launch(): Promise<EngineTransport>;
}

export interface EngineApi {
  listJobs(): Promise<readonly DownloadJob[]>;
  probeUrl(sourceUrl: string, destination: string): Promise<UrlProbe>;
  subscribeToEvents(listener: (event: DownloadEvent) => void): () => void;
}

type PendingRequest = {
  readonly resolve: (response: UiResponse) => void;
  readonly reject: (error: Error) => void;
};

export class EngineRejectedError extends Error {
  public constructor(
    public readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "EngineRejectedError";
  }
}

export class EngineClient implements EngineApi {
  readonly #transport: EngineTransport;
  readonly #pending = new Map<string, PendingRequest>();
  readonly #eventListeners = new Set<(event: DownloadEvent) => void>();
  readonly #unsubscribe: () => void;
  readonly #requestId: () => string;
  #closed = false;

  public constructor(
    transport: EngineTransport,
    requestId: () => string = () => globalThis.crypto.randomUUID(),
  ) {
    this.#transport = transport;
    this.#requestId = requestId;
    this.#unsubscribe = transport.subscribe((message) =>
      this.#receive(message),
    );
  }

  public async health(): Promise<string> {
    const response = await this.#request({ type: "health" });
    if (response.type !== "health") {
      throw new ProtocolViolationError(
        "health request received a non-health response",
      );
    }
    return response.engine_version;
  }

  public async listJobs(): Promise<readonly DownloadJob[]> {
    const response = await this.#request({ type: "list_jobs" });
    if (response.type !== "jobs") {
      throw new ProtocolViolationError(
        "list request received a non-jobs response",
      );
    }
    return response.jobs;
  }

  public async probeUrl(
    sourceUrl: string,
    destination: string,
  ): Promise<UrlProbe> {
    const response = await this.#request({
      type: "probe_url",
      source_url: sourceUrl,
      destination,
    });
    if (response.type !== "url_probe") {
      throw new ProtocolViolationError(
        "probe request received a non-probe response",
      );
    }
    return response;
  }

  public subscribeToEvents(
    listener: (event: DownloadEvent) => void,
  ): () => void {
    this.#eventListeners.add(listener);
    return () => this.#eventListeners.delete(listener);
  }

  public async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    this.#unsubscribe();
    const error = new Error("engine client closed");
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
    await this.#transport.close();
  }

  async #request(payload: UiRequest): Promise<UiResponse> {
    if (this.#closed) throw new Error("engine client is closed");
    const requestId = this.#requestId();
    const response = new Promise<UiResponse>((resolve, reject) => {
      this.#pending.set(requestId, { resolve, reject });
    });
    try {
      await this.#transport.send(createUiRequest(requestId, payload));
    } catch (error) {
      this.#pending.delete(requestId);
      throw error;
    }
    return response;
  }

  #receive(untrusted: unknown): void {
    let message;
    try {
      message = decodeUiMessage(untrusted);
    } catch (error) {
      const violation =
        error instanceof Error
          ? error
          : new ProtocolViolationError("invalid engine message");
      for (const pending of this.#pending.values()) pending.reject(violation);
      this.#pending.clear();
      return;
    }
    if (message.payload.type === "event") {
      for (const listener of this.#eventListeners)
        listener(message.payload.event);
      return;
    }
    const pending = this.#pending.get(message.request_id);
    if (pending === undefined) return;
    this.#pending.delete(message.request_id);
    const response = message.payload.response;
    if (response.type === "rejected") {
      pending.reject(new EngineRejectedError(response.code, response.message));
    } else {
      pending.resolve(response);
    }
  }
}
