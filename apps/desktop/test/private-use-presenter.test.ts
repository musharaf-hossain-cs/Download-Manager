import { describe, expect, it } from "vitest";

import {
  formatBytes,
  formatEta,
  presentJob,
} from "../src/private-use-presenter.js";
import { SegmentDetailController } from "../src/segment-detail.js";
import { jobFixture } from "./fixtures.js";

describe("private-use progress presentation", () => {
  it("shows status, progress, bytes, speed, and ETA from bounded samples", () => {
    const job = jobFixture({
      state: "downloading",
      downloaded_bytes: 500,
      total_bytes: 1_000,
    });
    const detail = new SegmentDetailController();
    detail.apply({
      job_id: job.id,
      at: 1_000,
      downloaded_bytes: 100,
      total_bytes: 1_000,
      segments: [],
    });
    const view = detail.apply({
      job_id: job.id,
      at: 3_000,
      downloaded_bytes: job.downloaded_bytes,
      total_bytes: job.total_bytes,
      segments: [],
    });

    expect(presentJob(job, view)).toEqual({
      status: "Downloading",
      progressLabel: "50% complete",
      progressPercent: 50,
      transferLabel: "500 B of 1000 B",
      speedLabel: "200 B/s",
      etaLabel: "3s",
    });
  });

  it("formats larger byte counts and unknown estimates compactly", () => {
    expect(formatBytes(1_572_864)).toBe("1.50 MiB");
    expect(formatEta(null)).toBe("—");
    expect(formatEta(3_661)).toBe("1h 2m");
  });
});
