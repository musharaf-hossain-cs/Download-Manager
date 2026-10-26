export type CloseBehavior = "minimize_to_tray" | "exit" | "ask";

export type TrayCommand =
  | { readonly type: "show" }
  | { readonly type: "hide" }
  | { readonly type: "quick_add" }
  | { readonly type: "set_speed_limit"; readonly bytesPerSecond: number | null }
  | { readonly type: "pause_all" }
  | { readonly type: "resume_all" }
  | { readonly type: "start_queue"; readonly queueId: string }
  | { readonly type: "stop_queue"; readonly queueId: string }
  | { readonly type: "exit" };

export type CloseChoice = Exclude<CloseBehavior, "ask"> | "cancel";
export type CloseResult = "hidden" | "exited" | "cancelled";

export interface TrayPort {
  showWindow(): Promise<void>;
  hideWindow(): Promise<void>;
  quickAdd(): Promise<void>;
  setSpeedLimit(bytesPerSecond: number | null): Promise<void>;
  pauseAll(): Promise<void>;
  resumeAll(): Promise<void>;
  startQueue(queueId: string): Promise<void>;
  stopQueue(queueId: string): Promise<void>;
  exit(): Promise<void>;
}

export interface CloseBehaviorStore {
  save(behavior: CloseBehavior): Promise<void>;
}

/** Owns system-tray commands and the persisted close-button policy. */
export class TrayController {
  #closeBehavior: CloseBehavior;

  public constructor(
    private readonly port: TrayPort,
    private readonly store: CloseBehaviorStore,
    persistedBehavior: unknown,
  ) {
    this.#closeBehavior = isCloseBehavior(persistedBehavior)
      ? persistedBehavior
      : "ask";
  }

  public get closeBehavior(): CloseBehavior {
    return this.#closeBehavior;
  }

  public async setCloseBehavior(behavior: CloseBehavior): Promise<void> {
    const previous = this.#closeBehavior;
    this.#closeBehavior = behavior;
    try {
      await this.store.save(behavior);
    } catch (error) {
      this.#closeBehavior = previous;
      throw error;
    }
  }

  public async requestClose(
    ask?: () => Promise<CloseChoice>,
  ): Promise<CloseResult> {
    const choice =
      this.#closeBehavior === "ask"
        ? await (ask?.() ?? Promise.resolve("cancel" as const))
        : this.#closeBehavior;
    if (choice === "cancel") return "cancelled";
    if (choice === "minimize_to_tray") {
      await this.port.hideWindow();
      return "hidden";
    }
    await this.port.exit();
    return "exited";
  }

  public async run(command: TrayCommand): Promise<void> {
    switch (command.type) {
      case "show":
        return this.port.showWindow();
      case "hide":
        return this.port.hideWindow();
      case "quick_add":
        return this.port.quickAdd();
      case "set_speed_limit":
        assertSpeedLimit(command.bytesPerSecond);
        return this.port.setSpeedLimit(command.bytesPerSecond);
      case "pause_all":
        return this.port.pauseAll();
      case "resume_all":
        return this.port.resumeAll();
      case "start_queue":
        return this.port.startQueue(assertQueueId(command.queueId));
      case "stop_queue":
        return this.port.stopQueue(assertQueueId(command.queueId));
      case "exit":
        return this.port.exit();
    }
  }
}

function isCloseBehavior(value: unknown): value is CloseBehavior {
  return value === "minimize_to_tray" || value === "exit" || value === "ask";
}

function assertSpeedLimit(value: number | null): void {
  if (value !== null && (!Number.isSafeInteger(value) || value <= 0)) {
    throw new RangeError("speed limit must be a positive integer or null");
  }
}

function assertQueueId(value: string): string {
  const queueId = value.trim();
  if (queueId.length === 0) throw new RangeError("queue id must not be empty");
  return queueId;
}
