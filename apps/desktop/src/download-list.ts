import type { EngineApi } from "./engine-client.js";
import type { ProgressEvent } from "./progress.js";
import type { DownloadJob } from "./protocol.js";

export type DownloadListState =
  | { readonly status: "loading"; readonly jobs: readonly DownloadJob[] }
  | { readonly status: "empty"; readonly jobs: readonly [] }
  | { readonly status: "ready"; readonly jobs: readonly DownloadJob[] }
  | {
      readonly status: "error";
      readonly jobs: readonly [];
      readonly message: string;
    };

/** Renderer-facing state model. It never reads SQLite directly. */
export class DownloadListController {
  #state: DownloadListState = { status: "loading", jobs: [] };

  public constructor(private readonly engine: Pick<EngineApi, "listJobs">) {}

  public get state(): DownloadListState {
    return this.#state;
  }

  public async load(): Promise<void> {
    this.#state = { status: "loading", jobs: [] };
    try {
      const jobs = await this.engine.listJobs();
      this.#state =
        jobs.length === 0
          ? { status: "empty", jobs: [] }
          : { status: "ready", jobs: [...jobs] };
    } catch (error) {
      this.#state = {
        status: "error",
        jobs: [],
        message:
          error instanceof Error ? error.message : "Unable to load downloads",
      };
    }
  }

  /** Applies an already-coalesced batch without exposing packet-rate events to the UI. */
  public applyProgress(events: readonly ProgressEvent[]): void {
    if (this.#state.status !== "ready" || events.length === 0) return;
    const latest = new Map(events.map((event) => [event.job_id, event]));
    this.#state = {
      status: "ready",
      jobs: this.#state.jobs.map((job) => {
        const event = latest.get(job.id);
        return event === undefined
          ? job
          : {
              ...job,
              downloaded_bytes: event.downloaded_bytes,
              total_bytes: event.total_bytes,
              updated_at: event.at,
            };
      }),
    };
  }
}
