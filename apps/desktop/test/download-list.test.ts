import { describe, expect, it } from "vitest";

import { DownloadListController } from "../src/download-list.js";
import { EngineRejectedError } from "../src/engine-client.js";
import { jobFixture } from "./fixtures.js";

const unusedControl = async () => {
  throw new Error("unexpected control request");
};

describe("persisted download list", () => {
  it("exposes loading until persisted jobs arrive", async () => {
    let resolveJobs:
      ((jobs: readonly ReturnType<typeof jobFixture>[]) => void) | undefined;
    const jobsPromise = new Promise<readonly ReturnType<typeof jobFixture>[]>(
      (resolve) => {
        resolveJobs = resolve;
      },
    );
    const controller = new DownloadListController({
      listJobs: () => jobsPromise,
      controlJob: unusedControl,
    });

    const loading = controller.load();
    expect(controller.state).toEqual({ status: "loading", jobs: [] });
    resolveJobs?.([jobFixture()]);
    await loading;

    expect(controller.state.status).toBe("ready");
    expect(controller.state.jobs[0]?.filename).toBe("archive.zip");
  });

  it("has a distinct empty state", async () => {
    const controller = new DownloadListController({
      listJobs: async () => [],
      controlJob: unusedControl,
    });
    await controller.load();
    expect(controller.state).toEqual({ status: "empty", jobs: [] });
  });

  it("has a safe error state when persistence cannot be read", async () => {
    const controller = new DownloadListController({
      controlJob: unusedControl,
      listJobs: async () => {
        throw new Error("database unavailable");
      },
    });
    await controller.load();
    expect(controller.state).toEqual({
      status: "error",
      jobs: [],
      message: "database unavailable",
    });
  });

  it("applies a coalesced progress batch to ready rows", async () => {
    const controller = new DownloadListController({
      controlJob: unusedControl,
      listJobs: async () => [
        jobFixture({ id: "job-a" }),
        jobFixture({ id: "job-b" }),
      ],
    });
    await controller.load();
    controller.applyProgress([
      {
        type: "progress",
        job_id: "job-b",
        downloaded_bytes: 750,
        total_bytes: 1_000,
        at: 1_700_000_000_250,
      },
    ]);
    expect(controller.state.jobs.map((job) => job.downloaded_bytes)).toEqual([
      0, 750,
    ]);
  });

  it("maps start, pause, resume, and stop to acknowledged states", async () => {
    const transitions = new Map([
      ["start", "probing"],
      ["pause", "pausing"],
      ["resume", "queued"],
      ["stop", "paused"],
    ] as const);
    const controller = new DownloadListController({
      listJobs: async () => [
        jobFixture({ id: "start", state: "created" }),
        jobFixture({ id: "pause", state: "downloading" }),
        jobFixture({ id: "resume", state: "paused" }),
        jobFixture({ id: "stop", state: "queued" }),
      ],
      controlJob: async (jobId, action) => ({
        jobId,
        state: transitions.get(action) ?? "failed",
        updatedAt: 1_700_000_000_500,
      }),
    });
    await controller.load();

    for (const action of ["start", "pause", "resume", "stop"] as const) {
      await expect(controller.control(action, action)).resolves.toBe(true);
    }
    expect(controller.state.jobs.map((job) => job.state)).toEqual([
      "probing",
      "pausing",
      "queued",
      "paused",
    ]);
    expect(
      controller.state.jobs.every(
        (job) => job.updated_at === 1_700_000_000_500,
      ),
    ).toBe(true);
  });

  it("rolls an optimistic state back when the engine rejects the control", async () => {
    let rejectControl: ((error: Error) => void) | undefined;
    const engineResponse = new Promise<never>((_resolve, reject) => {
      rejectControl = reject;
    });
    const controller = new DownloadListController({
      listJobs: async () => [jobFixture({ state: "downloading" })],
      controlJob: () => engineResponse,
    });
    await controller.load();

    const pending = controller.control(
      "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
      "pause",
    );
    expect(controller.state.jobs[0]?.state).toBe("pausing");
    rejectControl?.(
      new EngineRejectedError("invalid_job_action", "job is no longer active"),
    );

    await expect(pending).resolves.toBe(false);
    expect(controller.state.jobs[0]?.state).toBe("downloading");
    expect(controller.controlError).toEqual({
      jobId: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
      message: "job is no longer active",
    });
  });
});
