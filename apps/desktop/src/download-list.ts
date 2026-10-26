import type { EngineApi } from "./engine-client.js";
import type { ProgressEvent } from "./progress.js";
import type { DownloadJob, DownloadState, JobAction } from "./protocol.js";

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
  #controlError: { readonly jobId: string; readonly message: string } | null =
    null;

  public constructor(
    private readonly engine: Pick<EngineApi, "listJobs" | "controlJob">,
  ) {}

  public get state(): DownloadListState {
    return this.#state;
  }

  public get controlError(): {
    readonly jobId: string;
    readonly message: string;
  } | null {
    return this.#controlError;
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

  public async control(jobId: string, action: JobAction): Promise<boolean> {
    if (this.#state.status !== "ready") return false;
    const job = this.#state.jobs.find((candidate) => candidate.id === jobId);
    if (job === undefined) return false;
    const optimistic = controlTarget(job.state, action);
    if (optimistic === null) return false;

    this.#controlError = null;
    this.#replaceJob(jobId, (current) => ({ ...current, state: optimistic }));
    try {
      const acknowledged = await this.engine.controlJob(jobId, action);
      this.#replaceJob(jobId, (current) => ({
        ...current,
        state: acknowledged.state,
        updated_at: acknowledged.updatedAt,
      }));
      return true;
    } catch (error) {
      this.#replaceJob(jobId, (current) => ({ ...current, state: job.state }));
      this.#controlError = {
        jobId,
        message: error instanceof Error ? error.message : "Job control failed",
      };
      return false;
    }
  }

  #replaceJob(jobId: string, replace: (job: DownloadJob) => DownloadJob): void {
    if (this.#state.status !== "ready") return;
    this.#state = {
      status: "ready",
      jobs: this.#state.jobs.map((job) =>
        job.id === jobId ? replace(job) : job,
      ),
    };
  }
}

function controlTarget(
  current: DownloadState,
  action: JobAction,
): DownloadState | null {
  if (current === "created" && action === "start") return "probing";
  if (current === "queued" && action === "start") return "connecting";
  if (current === "downloading" && (action === "pause" || action === "stop"))
    return "pausing";
  if (current === "queued" && (action === "pause" || action === "stop"))
    return "paused";
  if (current === "paused" && action === "resume") return "queued";
  return null;
}
