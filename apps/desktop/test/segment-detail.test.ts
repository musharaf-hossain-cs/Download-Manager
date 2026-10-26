import { describe, expect, it } from "vitest";

import {
  SegmentDetailController,
  type SegmentDetailSnapshot,
} from "../src/segment-detail.js";

const snapshot = (
  overrides: Partial<SegmentDetailSnapshot> = {},
): SegmentDetailSnapshot => ({
  job_id: "job-a",
  at: 1_000,
  downloaded_bytes: 0,
  total_bytes: 1_000,
  segments: [
    {
      id: "left",
      start: 0,
      end: 499,
      durable_offset: 0,
      worker_state: "receiving",
    },
    {
      id: "right",
      start: 500,
      end: 999,
      durable_offset: 500,
      worker_state: "receiving",
    },
  ],
  ...overrides,
});

describe("segment detail view", () => {
  it("turns fake event snapshots into per-connection progress, speed, and ETA", () => {
    const controller = new SegmentDetailController();
    controller.apply(snapshot());
    const view = controller.apply(
      snapshot({
        at: 3_000,
        downloaded_bytes: 300,
        segments: [
          {
            id: "left",
            start: 0,
            end: 499,
            durable_offset: 200,
            worker_state: "receiving",
          },
          {
            id: "right",
            start: 500,
            end: 999,
            durable_offset: 600,
            worker_state: "waiting",
          },
        ],
      }),
    );

    expect(view).toMatchInlineSnapshot(`
      {
        "downloaded_bytes": 300,
        "eta_seconds": 5,
        "job_id": "job-a",
        "progress_percent": 30,
        "segments": [
          {
            "downloaded_bytes": 200,
            "id": "left",
            "progress_percent": 40,
            "range_end": 499,
            "range_start": 0,
            "size_bytes": 500,
            "speed_bytes_per_second": 100,
            "worker_state": "receiving",
          },
          {
            "downloaded_bytes": 100,
            "id": "right",
            "progress_percent": 20,
            "range_end": 999,
            "range_start": 500,
            "size_bytes": 500,
            "speed_bytes_per_second": 50,
            "worker_state": "waiting",
          },
        ],
        "speed_bytes_per_second": 150,
        "total_bytes": 1000,
      }
    `);
  });

  it("reports a zero ETA when the download is complete, even at zero speed", () => {
    const controller = new SegmentDetailController();
    controller.apply(snapshot({ downloaded_bytes: 1_000 }));
    const view = controller.apply(
      snapshot({ at: 2_000, downloaded_bytes: 1_000 }),
    );

    expect(view.speed_bytes_per_second).toBe(0);
    expect(view.eta_seconds).toBe(0);
    expect(view.progress_percent).toBe(100);
  });

  it("keeps ETA unknown for unknown totals, first samples, and stalled work", () => {
    const controller = new SegmentDetailController();
    const first = controller.apply(snapshot({ downloaded_bytes: 100 }));
    const stalled = controller.apply(
      snapshot({ at: 2_000, downloaded_bytes: 100 }),
    );
    const unknownTotal = controller.apply(
      snapshot({
        at: 3_000,
        downloaded_bytes: 200,
        total_bytes: null,
      }),
    );

    expect([
      first.eta_seconds,
      stalled.eta_seconds,
      unknownTotal.eta_seconds,
    ]).toEqual([null, null, null]);
    expect(unknownTotal.progress_percent).toBeNull();
  });

  it("rejects offsets outside their inclusive segment range", () => {
    const controller = new SegmentDetailController();
    expect(() =>
      controller.apply(
        snapshot({
          segments: [
            {
              id: "bad",
              start: 10,
              end: 19,
              durable_offset: 21,
              worker_state: "receiving",
            },
          ],
        }),
      ),
    ).toThrow("invalid segment range for bad");
  });
});
