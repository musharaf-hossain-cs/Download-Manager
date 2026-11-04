import type { EngineTransport } from "./engine-client.js";
import type { UiRequestEnvelope } from "./protocol.js";

export type TauriInvoke = (
  command: string,
  arguments_: Record<string, unknown>,
) => Promise<unknown>;

/** Adapts one-response Tauri commands to the existing typed engine transport. */
export class TauriEngineTransport implements EngineTransport {
  readonly #listeners = new Set<(message: unknown) => void>();
  #closed = false;

  public constructor(private readonly invoke: TauriInvoke) {}

  public async send(message: UiRequestEnvelope): Promise<void> {
    if (this.#closed) throw new Error("desktop backend transport is closed");
    const encoded = await this.invoke("backend_request", {
      request: JSON.stringify(message),
    });
    if (typeof encoded !== "string") {
      throw new TypeError("desktop backend returned a non-text response");
    }
    const response: unknown = JSON.parse(encoded);
    for (const listener of this.#listeners) listener(response);
  }

  public subscribe(listener: (message: unknown) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  public close(): void {
    this.#closed = true;
    this.#listeners.clear();
  }
}
