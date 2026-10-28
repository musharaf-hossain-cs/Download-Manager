import { describe, expect, it } from "vitest";

import {
  QueueController,
  type QueueJobOperation,
  type QueuePort,
  type QueueRunAction,
  type QueueSnapshot,
} from "../src/queues.js";
import type { DownloadJob } from "../src/protocol.js";
import { jobFixture } from "./fixtures.js";

describe("queue UI", () => {
  it("moves and copies deduplicated jobs at an exact target position", async () => {
    const port = new MemoryQueuePort([
      queue("Work", [
        job("one", "queued", "Work"),
        job("two", "paused", "Work"),
      ]),
      queue("Night", [job("anchor", "queued", "Night")]),
    ]);
    const controller = new QueueController(port);
    await controller.load();

    await expect(
      controller.transferJobs(
        "move",
        ["two", "missing", "two"],
        " Night ",
        "anchor",
      ),
    ).resolves.toEqual({ succeeded: ["two"], skipped: ["missing"] });
    expect(queueIds(controller, "Work")).toEqual(["one"]);
    expect(queueIds(controller, "Night")).toEqual(["two", "anchor"]);

    await expect(
      controller.transferJobs("copy", ["one"], "Night"),
    ).resolves.toEqual({ succeeded: ["one"], skipped: [] });
    expect(queueIds(controller, "Work")).toEqual(["one"]);
    expect(queueIds(controller, "Night")).toEqual([
      "two",
      "anchor",
      "one-copy",
    ]);
    expect(port.calls).toEqual(["move:two:Night:anchor", "copy:one:Night:end"]);
  });

  it("accepts only an exact full reorder and a target-owned insertion anchor", async () => {
    const port = new MemoryQueuePort([
      queue("Work", [
        job("one", "queued", "Work"),
        job("two", "queued", "Work"),
        job("three", "paused", "Work"),
      ]),
      queue("Night", [job("night", "queued", "Night")]),
    ]);
    const controller = new QueueController(port);
    await controller.load();

    await expect(
      controller.reorderJobs("Work", ["three", "one", "two"]),
    ).resolves.toBeUndefined();
    expect(queueIds(controller, "Work")).toEqual(["three", "one", "two"]);
    await expect(
      controller.reorderJobs("Work", ["three", "three", "one"]),
    ).rejects.toThrow("every queue job exactly once");
    await expect(
      controller.transferJobs("move", ["one"], "Night", "two"),
    ).rejects.toThrow("insertion anchor");
    expect(port.calls).toEqual(["reorder:Work:three,one,two"]);
  });

  it("starts and stops only eligible jobs from mixed queue states", async () => {
    const port = new MemoryQueuePort([
      queue("Mixed", [
        job("queued", "queued", "Mixed"),
        job("paused", "paused", "Mixed"),
        job("active", "downloading", "Mixed"),
        job("retry", "retry_waiting", "Mixed"),
        job("done", "completed", "Mixed"),
      ]),
    ]);
    const controller = new QueueController(port);
    await controller.load();

    await expect(controller.setQueueRunning("Mixed", "start")).resolves.toEqual(
      {
        succeeded: ["queued", "paused"],
        skipped: ["active", "retry", "done"],
      },
    );
    await expect(controller.setQueueRunning("Mixed", "stop")).resolves.toEqual({
      succeeded: ["queued", "paused", "active", "retry"],
      skipped: ["done"],
    });
    expect(port.calls).toEqual([
      "start:Mixed:queued,paused",
      "stop:Mixed:queued,paused,active,retry",
    ]);
    expect(queueStates(controller, "Mixed")).toEqual([
      "paused",
      "paused",
      "pausing",
      "paused",
      "completed",
    ]);
  });

  it("preserves the prior snapshot and reports a rejected mutation", async () => {
    const initial = [queue("Work", [job("one", "queued", "Work")])];
    const port = new MemoryQueuePort(initial);
    const controller = new QueueController(port);
    await controller.load();
    port.failure = new Error("queue store busy");

    await expect(
      controller.transferJobs("move", ["one"], "Work"),
    ).rejects.toThrow("queue store busy");
    expect(queueIds(controller, "Work")).toEqual(["one"]);
    expect(controller.mutationError).toBe("queue store busy");
  });
});

class MemoryQueuePort implements QueuePort {
  public readonly calls: string[] = [];
  public failure: Error | null = null;
  #queues: QueueSnapshot[];

  public constructor(queues: readonly QueueSnapshot[]) {
    this.#queues = cloneQueues(queues);
  }

  public async listQueues(): Promise<readonly QueueSnapshot[]> {
    return cloneQueues(this.#queues);
  }

  public async transferJobs(
    operation: QueueJobOperation,
    jobIds: readonly string[],
    targetQueue: string,
    beforeJobId: string | null,
  ): Promise<readonly QueueSnapshot[]> {
    this.#failIfRequested();
    this.calls.push(
      `${operation}:${jobIds.join(",")}:${targetQueue}:${beforeJobId ?? "end"}`,
    );
    const selected = this.#queues
      .flatMap((entry) => entry.jobs)
      .filter((entry) => jobIds.includes(entry.id));
    if (operation === "move") {
      this.#queues = this.#queues.map((entry) => ({
        ...entry,
        jobs: entry.jobs.filter((candidate) => !jobIds.includes(candidate.id)),
      }));
    }
    this.#queues = this.#queues.map((entry) => {
      if (entry.name !== targetQueue) return entry;
      const inserted = selected.map((entry) => ({
        ...entry,
        id: operation === "copy" ? `${entry.id}-copy` : entry.id,
        queue: targetQueue,
        state: operation === "copy" ? ("queued" as const) : entry.state,
      }));
      const jobs = [...entry.jobs];
      const index =
        beforeJobId === null
          ? jobs.length
          : jobs.findIndex((candidate) => candidate.id === beforeJobId);
      jobs.splice(index, 0, ...inserted);
      return { ...entry, jobs };
    });
    return cloneQueues(this.#queues);
  }

  public async reorderJobs(
    queueName: string,
    orderedJobIds: readonly string[],
  ): Promise<readonly QueueSnapshot[]> {
    this.#failIfRequested();
    this.calls.push(`reorder:${queueName}:${orderedJobIds.join(",")}`);
    this.#queues = this.#queues.map((entry) =>
      entry.name === queueName
        ? {
            ...entry,
            jobs: orderedJobIds.map((jobId) =>
              entry.jobs.find((job) => job.id === jobId)!,
            ),
          }
        : entry,
    );
    return cloneQueues(this.#queues);
  }

  public async setQueueRunning(
    queueName: string,
    action: QueueRunAction,
    eligibleJobIds: readonly string[],
  ): Promise<readonly QueueSnapshot[]> {
    this.#failIfRequested();
    this.calls.push(`${action}:${queueName}:${eligibleJobIds.join(",")}`);
    this.#queues = this.#queues.map((entry) =>
      entry.name === queueName
        ? {
            ...entry,
            running: action === "start",
            jobs: entry.jobs.map((job) =>
              eligibleJobIds.includes(job.id)
                ? { ...job, state: targetState(job.state, action) }
                : job,
            ),
          }
        : entry,
    );
    return cloneQueues(this.#queues);
  }

  #failIfRequested(): void {
    if (this.failure !== null) throw this.failure;
  }
}

function targetState(
  state: DownloadJob["state"],
  action: QueueRunAction,
): DownloadJob["state"] {
  if (action === "start") return state === "paused" ? "queued" : "connecting";
  return state === "downloading" ? "pausing" : "paused";
}

function queue(name: string, jobs: readonly DownloadJob[]): QueueSnapshot {
  return { name, running: false, jobs };
}

function job(
  id: string,
  state: DownloadJob["state"],
  queueName: string,
): DownloadJob {
  return jobFixture({ id, state, queue: queueName });
}

function cloneQueues(queues: readonly QueueSnapshot[]): QueueSnapshot[] {
  return queues.map((entry) => ({
    ...entry,
    jobs: entry.jobs.map((job) => ({ ...job })),
  }));
}

function readyQueues(controller: QueueController): readonly QueueSnapshot[] {
  if (controller.state.status !== "ready") throw new Error("expected queues");
  return controller.state.queues;
}

function queueIds(controller: QueueController, name: string): string[] {
  return readyQueues(controller)
    .find((entry) => entry.name === name)!
    .jobs.map((job) => job.id);
}

function queueStates(
  controller: QueueController,
  name: string,
): DownloadJob["state"][] {
  return readyQueues(controller)
    .find((entry) => entry.name === name)!
    .jobs.map((job) => job.state);
}
