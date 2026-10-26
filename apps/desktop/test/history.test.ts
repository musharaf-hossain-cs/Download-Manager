import { describe, expect, it } from "vitest";

import { HistoryController } from "../src/history.js";
import { jobFixture } from "./fixtures.js";

const DAY = 86_400_000;

describe("history UI", () => {
  it("searches and filters completed history without including active jobs", () => {
    const controller = new HistoryController(
      [
        jobFixture({
          id: "manual",
          state: "completed",
          filename: "Project Manual.pdf",
          category: "Documents",
          completed_at: 10 * DAY,
        }),
        jobFixture({
          id: "music",
          state: "completed",
          filename: "theme.flac",
          category: "Music",
          completed_at: 20 * DAY,
        }),
        jobFixture({ id: "active", state: "downloading" }),
      ],
      { clearRecords: async () => undefined },
    );

    controller.setFilter({ query: "manual", category: "Documents" });
    expect(controller.state.visible.map((record) => record.id)).toEqual([
      "manual",
    ]);

    controller.setFilter({
      query: "",
      category: null,
      completedAfter: 15 * DAY,
    });
    expect(controller.state.visible.map((record) => record.id)).toEqual([
      "music",
    ]);
  });

  it("applies the configured retention cutoff to records only", async () => {
    const files = new Set(["C:\\Downloads\\old.zip", "C:\\Downloads\\new.zip"]);
    const cleared: string[][] = [];
    const controller = new HistoryController(
      [
        jobFixture({ id: "old", state: "completed", completed_at: 5 * DAY }),
        jobFixture({ id: "new", state: "completed", completed_at: 25 * DAY }),
      ],
      {
        clearRecords: async (ids) => {
          cleared.push([...ids]);
        },
      },
    );

    controller.setRetentionDays(10);
    await expect(controller.applyRetention(30 * DAY)).resolves.toEqual(["old"]);
    expect(cleared).toEqual([["old"]]);
    expect(controller.state.records.map((record) => record.id)).toEqual([
      "new",
    ]);
    expect(files).toEqual(
      new Set(["C:\\Downloads\\old.zip", "C:\\Downloads\\new.zip"]),
    );
  });

  it("clears only the filtered history records and never targets files", async () => {
    const cleared: string[][] = [];
    const controller = new HistoryController(
      [
        jobFixture({
          id: "document",
          state: "completed",
          category: "Documents",
        }),
        jobFixture({ id: "video", state: "completed", category: "Video" }),
      ],
      {
        clearRecords: async (ids) => {
          cleared.push([...ids]);
        },
      },
    );
    controller.setFilter({ category: "Documents" });

    await expect(controller.clearVisible()).resolves.toEqual(["document"]);
    expect(cleared).toEqual([["document"]]);
    expect(controller.state.records.map((record) => record.id)).toEqual([
      "video",
    ]);
  });

  it("rejects unsafe retention values", () => {
    const controller = new HistoryController([], {
      clearRecords: async () => undefined,
    });
    expect(() => controller.setRetentionDays(0)).toThrow(RangeError);
    expect(() => controller.setRetentionDays(1.5)).toThrow(RangeError);
  });
});
