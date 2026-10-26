import { describe, expect, it } from "vitest";

import { DownloadListController } from "../src/download-list.js";
import { jobFixture } from "./fixtures.js";

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
    });

    const loading = controller.load();
    expect(controller.state).toEqual({ status: "loading", jobs: [] });
    resolveJobs?.([jobFixture()]);
    await loading;

    expect(controller.state.status).toBe("ready");
    expect(controller.state.jobs[0]?.filename).toBe("archive.zip");
  });

  it("has a distinct empty state", async () => {
    const controller = new DownloadListController({ listJobs: async () => [] });
    await controller.load();
    expect(controller.state).toEqual({ status: "empty", jobs: [] });
  });

  it("has a safe error state when persistence cannot be read", async () => {
    const controller = new DownloadListController({
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
});
