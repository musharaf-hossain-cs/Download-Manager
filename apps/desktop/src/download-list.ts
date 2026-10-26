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

export interface RemovalConfirmation {
  readonly jobId: string;
  readonly action: "remove_record" | "delete_file";
  readonly confirmedPath: string | null;
}

/** Renderer-facing state model. It never reads SQLite directly. */
export class DownloadListController {
  #state: DownloadListState = { status: "loading", jobs: [] };
  #controlError: { readonly jobId: string; readonly message: string } | null =
    null;

  public constructor(
    private readonly engine: Pick<
      EngineApi,
      "listJobs" | "controlJob" | "removeJob"
    >,
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

  public prepareRemoval(
    jobId: string,
    deleteFile: boolean,
  ): RemovalConfirmation | null {
    if (this.#state.status !== "ready") return null;
    const job = this.#state.jobs.find((candidate) => candidate.id === jobId);
    if (job === undefined || !isRemovable(job.state)) return null;
    return {
      jobId,
      action: deleteFile ? "delete_file" : "remove_record",
      confirmedPath: deleteFile ? localFilePath(job) : null,
    };
  }

  public async remove(confirmation: RemovalConfirmation): Promise<boolean> {
    const expected = this.prepareRemoval(
      confirmation.jobId,
      confirmation.action === "delete_file",
    );
    if (
      expected === null ||
      expected.action !== confirmation.action ||
      expected.confirmedPath !== confirmation.confirmedPath
    ) {
      this.#controlError = {
        jobId: confirmation.jobId,
        message: "Removal confirmation does not match the selected job",
      };
      return false;
    }
    try {
      await this.engine.removeJob(
        confirmation.jobId,
        confirmation.action,
        confirmation.confirmedPath,
      );
      this.#removeJob(confirmation.jobId);
      return true;
    } catch (error) {
      this.#setControlError(confirmation.jobId, error);
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

  #removeJob(jobId: string): void {
    if (this.#state.status !== "ready") return;
    const jobs = this.#state.jobs.filter((job) => job.id !== jobId);
    this.#state =
      jobs.length === 0
        ? { status: "empty", jobs: [] }
        : { status: "ready", jobs };
  }

  #setControlError(jobId: string, error: unknown): void {
    this.#controlError = {
      jobId,
      message: error instanceof Error ? error.message : "Job action failed",
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
  if (
    action === "cancel" &&
    !["completed", "cancelled", "missing_local_file"].includes(current)
  )
    return "cancelled";
  return null;
}

function isRemovable(state: DownloadState): boolean {
  return [
    "created",
    "paused",
    "completed",
    "needs_authentication",
    "needs_refresh",
    "blocked",
    "failed",
    "cancelled",
    "missing_local_file",
  ].includes(state);
}

function localFilePath(job: DownloadJob): string {
  const separator = job.destination.endsWith("\\") ? "" : "\\";
  const suffix = job.state === "completed" ? "" : ".part";
  return `${job.destination}${separator}${job.filename}${suffix}`;
}
