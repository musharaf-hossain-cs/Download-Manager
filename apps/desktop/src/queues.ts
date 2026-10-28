import type { DownloadJob, DownloadState } from "./protocol.js";

export interface QueueSnapshot {
  readonly name: string;
  readonly running: boolean;
  readonly jobs: readonly DownloadJob[];
}

export type QueueJobOperation = "move" | "copy";
export type QueueRunAction = "start" | "stop";

export interface QueueMutationResult {
  readonly succeeded: readonly string[];
  readonly skipped: readonly string[];
}

/** Engine-facing queue commands; every mutation returns the authoritative ordered snapshot. */
export interface QueuePort {
  listQueues(): Promise<readonly QueueSnapshot[]>;
  transferJobs(
    operation: QueueJobOperation,
    jobIds: readonly string[],
    targetQueue: string,
    beforeJobId: string | null,
  ): Promise<readonly QueueSnapshot[]>;
  reorderJobs(
    queueName: string,
    orderedJobIds: readonly string[],
  ): Promise<readonly QueueSnapshot[]>;
  setQueueRunning(
    queueName: string,
    action: QueueRunAction,
    eligibleJobIds: readonly string[],
  ): Promise<readonly QueueSnapshot[]>;
}

export type QueueUiState =
  | { readonly status: "loading"; readonly queues: readonly [] }
  | { readonly status: "empty"; readonly queues: readonly [] }
  | { readonly status: "ready"; readonly queues: readonly QueueSnapshot[] }
  | {
      readonly status: "error";
      readonly queues: readonly [];
      readonly message: string;
    };

/** Renderer-neutral model for queue ordering, membership, and manual controls. */
export class QueueController {
  #state: QueueUiState = { status: "loading", queues: [] };
  #mutationError: string | null = null;

  public constructor(private readonly port: QueuePort) {}

  public get state(): QueueUiState {
    return this.#state;
  }

  public get mutationError(): string | null {
    return this.#mutationError;
  }

  public async load(): Promise<void> {
    this.#state = { status: "loading", queues: [] };
    try {
      this.#replaceQueues(await this.port.listQueues());
    } catch (error) {
      this.#state = {
        status: "error",
        queues: [],
        message: errorMessage(error, "Unable to load queues"),
      };
    }
  }

  public async transferJobs(
    operation: QueueJobOperation,
    jobIds: readonly string[],
    targetQueue: string,
    beforeJobId: string | null = null,
  ): Promise<QueueMutationResult> {
    const queues = this.#readyQueues();
    const target = requireQueue(queues, targetQueue);
    if (
      beforeJobId !== null &&
      !target.jobs.some((job) => job.id === beforeJobId)
    ) {
      throw new RangeError("insertion anchor is not in the target queue");
    }
    const existing = new Set(
      queues.flatMap((queue) => queue.jobs.map((job) => job.id)),
    );
    const { accepted, skipped } = partitionKnown(jobIds, existing);
    if (accepted.length === 0) return { succeeded: [], skipped };

    await this.#mutate(() =>
      this.port.transferJobs(operation, accepted, target.name, beforeJobId),
    );
    return { succeeded: accepted, skipped };
  }

  public async reorderJobs(
    queueName: string,
    orderedJobIds: readonly string[],
  ): Promise<void> {
    const queue = requireQueue(this.#readyQueues(), queueName);
    const expected = queue.jobs.map((job) => job.id);
    if (!isExactPermutation(orderedJobIds, expected)) {
      throw new RangeError(
        "job order must contain every queue job exactly once",
      );
    }
    await this.#mutate(() => this.port.reorderJobs(queue.name, orderedJobIds));
  }

  public async setQueueRunning(
    queueName: string,
    action: QueueRunAction,
  ): Promise<QueueMutationResult> {
    const queue = requireQueue(this.#readyQueues(), queueName);
    const succeeded: string[] = [];
    const skipped: string[] = [];
    for (const job of queue.jobs) {
      (isEligible(job.state, action) ? succeeded : skipped).push(job.id);
    }
    if (succeeded.length === 0) return { succeeded, skipped };

    await this.#mutate(() =>
      this.port.setQueueRunning(queue.name, action, succeeded),
    );
    return { succeeded, skipped };
  }

  #readyQueues(): readonly QueueSnapshot[] {
    if (this.#state.status !== "ready") {
      throw new Error("queues must be loaded before they can be changed");
    }
    return this.#state.queues;
  }

  async #mutate(
    operation: () => Promise<readonly QueueSnapshot[]>,
  ): Promise<void> {
    this.#mutationError = null;
    try {
      this.#replaceQueues(await operation());
    } catch (error) {
      this.#mutationError = errorMessage(error, "Queue operation failed");
      throw error;
    }
  }

  #replaceQueues(queues: readonly QueueSnapshot[]): void {
    const normalized = normalizeSnapshots(queues);
    this.#state =
      normalized.length === 0
        ? { status: "empty", queues: [] }
        : { status: "ready", queues: normalized };
  }
}

function normalizeSnapshots(
  snapshots: readonly QueueSnapshot[],
): readonly QueueSnapshot[] {
  const names = new Set<string>();
  const jobIds = new Set<string>();
  return snapshots.map((snapshot) => {
    const name = snapshot.name.trim();
    if (
      name !== snapshot.name ||
      name.length === 0 ||
      [...name].length > 256 ||
      names.has(name)
    ) {
      throw new Error("engine returned an invalid queue snapshot");
    }
    names.add(name);
    const jobs = snapshot.jobs.map((job) => {
      if (jobIds.has(job.id) || job.queue !== name) {
        throw new Error("engine returned an invalid queue job assignment");
      }
      jobIds.add(job.id);
      return { ...job };
    });
    return { name, running: snapshot.running, jobs };
  });
}

function requireQueue(
  queues: readonly QueueSnapshot[],
  value: string,
): QueueSnapshot {
  const name = value.trim();
  const queue = queues.find((candidate) => candidate.name === name);
  if (queue === undefined) throw new RangeError("unknown queue");
  return queue;
}

function partitionKnown(
  jobIds: readonly string[],
  existing: ReadonlySet<string>,
): { accepted: string[]; skipped: string[] } {
  const seen = new Set<string>();
  const accepted: string[] = [];
  const skipped: string[] = [];
  for (const jobId of jobIds) {
    if (seen.has(jobId)) continue;
    seen.add(jobId);
    (existing.has(jobId) ? accepted : skipped).push(jobId);
  }
  return { accepted, skipped };
}

function isExactPermutation(
  requested: readonly string[],
  expected: readonly string[],
): boolean {
  return (
    requested.length === expected.length &&
    new Set(requested).size === requested.length &&
    expected.every((jobId) => requested.includes(jobId))
  );
}

function isEligible(state: DownloadState, action: QueueRunAction): boolean {
  if (action === "start") return state === "queued" || state === "paused";
  return ["queued", "connecting", "downloading", "retry_waiting"].includes(
    state,
  );
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}
