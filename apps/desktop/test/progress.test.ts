import { describe, expect, it } from "vitest";

import {
  PROGRESS_UPDATE_INTERVAL_MS,
  ProgressCoalescer,
  type ProgressEvent,
  type ProgressScheduler,
} from "../src/progress.js";

class ManualScheduler implements ProgressScheduler {
  callbacks: (() => void)[] = [];
  delays: number[] = [];

  public schedule(delay: number, callback: () => void): unknown {
    this.delays.push(delay);
    this.callbacks.push(callback);
    return callback;
  }

  public cancel(handle: unknown): void {
    this.callbacks = this.callbacks.filter((callback) => callback !== handle);
  }

  public runNext(): void {
    this.callbacks.shift()?.();
  }
}

describe("progress coalescing", () => {
  it("bounds high-frequency input to one latest-value batch at 4 Hz", () => {
    const scheduler = new ManualScheduler();
    const published: (readonly ProgressEvent[])[] = [];
    const coalescer = new ProgressCoalescer(
      (events) => published.push(events),
      scheduler,
    );

    for (let downloaded = 1; downloaded <= 20_000; downloaded += 1) {
      coalescer.push({
        type: "progress",
        job_id: "job-a",
        downloaded_bytes: downloaded,
        total_bytes: 20_000,
        at: downloaded,
      });
    }

    expect(scheduler.callbacks).toHaveLength(1);
    expect(scheduler.delays).toEqual([PROGRESS_UPDATE_INTERVAL_MS]);
    expect(published).toHaveLength(0);
    scheduler.runNext();
    expect(published).toHaveLength(1);
    expect(published[0]).toEqual([
      {
        type: "progress",
        job_id: "job-a",
        downloaded_bytes: 20_000,
        total_bytes: 20_000,
        at: 20_000,
      },
    ]);
  });

  it("coalesces independently by job and ignores non-progress events", () => {
    const scheduler = new ManualScheduler();
    const published: (readonly ProgressEvent[])[] = [];
    const coalescer = new ProgressCoalescer(
      (events) => published.push(events),
      scheduler,
    );
    coalescer.push({
      type: "state_changed",
      transition: {
        job_id: "job-a",
        from: "queued",
        to: "connecting",
        at: 1,
        reason: null,
      },
    });
    for (const jobId of ["job-a", "job-b"]) {
      coalescer.push({
        type: "progress",
        job_id: jobId,
        downloaded_bytes: 5,
        total_bytes: 10,
        at: 2,
      });
    }
    scheduler.runNext();
    expect(published[0]?.map((event) => event.job_id)).toEqual([
      "job-a",
      "job-b",
    ]);
  });
});
