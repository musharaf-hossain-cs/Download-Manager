import { EngineClient, type EngineLauncher } from "./engine-client.js";

export type DesktopShellStatus = "stopped" | "starting" | "running" | "closing";

/** Owns the engine child-process boundary used by the future Tauri adapter. */
export class DesktopShell {
  #status: DesktopShellStatus = "stopped";
  #client: EngineClient | null = null;

  public constructor(private readonly launcher: EngineLauncher) {}

  public get status(): DesktopShellStatus {
    return this.#status;
  }

  public get engine(): EngineClient {
    if (this.#client === null || this.#status !== "running") {
      throw new Error("desktop shell is not running");
    }
    return this.#client;
  }

  public async start(): Promise<string> {
    if (this.#status !== "stopped")
      throw new Error("desktop shell already started");
    this.#status = "starting";
    let transport;
    try {
      transport = await this.launcher.launch();
    } catch (error) {
      this.#status = "stopped";
      throw error;
    }
    const client = new EngineClient(transport);
    this.#client = client;
    try {
      const version = await client.health();
      this.#status = "running";
      return version;
    } catch (error) {
      await client.close();
      this.#client = null;
      this.#status = "stopped";
      throw error;
    }
  }

  public async close(): Promise<void> {
    if (this.#status === "stopped") return;
    this.#status = "closing";
    const client = this.#client;
    this.#client = null;
    await client?.close();
    this.#status = "stopped";
  }
}
