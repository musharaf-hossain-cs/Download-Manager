import type { EngineApi } from "./engine-client.js";
import type { DownloadEvent } from "./protocol.js";

export const PROGRESS_UPDATE_INTERVAL_MS = 250;

export interface ProgressScheduler {
  schedule(delayMilliseconds: number, callback: () => void): unknown;
  cancel(handle: unknown): void;
}

const timeoutScheduler: ProgressScheduler = {
  schedule: (delay, callback) => globalThis.setTimeout(callback, delay),
  cancel: (handle) => globalThis.clearTimeout(handle as number),
};

export type ProgressEvent = Extract<
  DownloadEvent,
  { readonly type: "progress" }
>;

/** Coalesces arbitrary engine frequency into one latest-value batch at 4 Hz. */
export class ProgressCoalescer {
  readonly #pending = new Map<string, ProgressEvent>();
  #scheduled: unknown = null;
  #disposed = false;

  public constructor(
    private readonly publish: (events: readonly ProgressEvent[]) => void,
    private readonly scheduler: ProgressScheduler = timeoutScheduler,
  ) {}

  public push(event: DownloadEvent): void {
    if (this.#disposed || event.type !== "progress") return;
    this.#pending.set(event.job_id, event);
    if (this.#scheduled === null) {
      this.#scheduled = this.scheduler.schedule(
        PROGRESS_UPDATE_INTERVAL_MS,
        () => this.#flush(),
      );
    }
  }

  public dispose(): void {
    this.#disposed = true;
    if (this.#scheduled !== null) this.scheduler.cancel(this.#scheduled);
    this.#scheduled = null;
    this.#pending.clear();
  }

  #flush(): void {
    this.#scheduled = null;
    if (this.#disposed || this.#pending.size === 0) return;
    const events = [...this.#pending.values()];
    this.#pending.clear();
    this.publish(events);
  }
}

/** Connects the engine event stream to a coalesced renderer update callback. */
export function subscribeToCoalescedProgress(
  engine: Pick<EngineApi, "subscribeToEvents">,
  publish: (events: readonly ProgressEvent[]) => void,
  scheduler?: ProgressScheduler,
): () => void {
  const coalescer = new ProgressCoalescer(publish, scheduler);
  const unsubscribe = engine.subscribeToEvents((event) =>
    coalescer.push(event),
  );
  return () => {
    unsubscribe();
    coalescer.dispose();
  };
}
