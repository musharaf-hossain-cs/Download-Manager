import { describe, expect, it } from "vitest";

import { VirtualDownloadList, sortJobs } from "../src/virtual-list.js";
import { jobFixture } from "./fixtures.js";

function tenThousandJobs() {
  return Array.from({ length: 10_000 }, (_, index) =>
    jobFixture({
      id: `job-${index.toString().padStart(5, "0")}`,
      filename: `download-${(9_999 - index).toString().padStart(5, "0")}.bin`,
      downloaded_bytes: index,
      total_bytes: 10_000,
      created_at: index,
      updated_at: index,
    }),
  );
}

describe("virtualized sortable download list", () => {
  it("sorts stably without mutating persisted order", () => {
    const original = [
      jobFixture({ id: "first", filename: "same.bin", created_at: 1 }),
      jobFixture({ id: "second", filename: "alpha.bin", created_at: 2 }),
      jobFixture({ id: "third", filename: "same.bin", created_at: 3 }),
    ];
    const sorted = sortJobs(original, {
      column: "filename",
      direction: "ascending",
    });
    expect(sorted.map((job) => job.id)).toEqual(["second", "first", "third"]);
    expect(original.map((job) => job.id)).toEqual(["first", "second", "third"]);
  });

  it("projects only a bounded viewport from 10,000 sorted rows", () => {
    const started = performance.now();
    const list = new VirtualDownloadList(
      tenThousandJobs(),
      { column: "filename", direction: "ascending" },
      32,
      3,
    );
    const view = list.window(160_000, 640);
    const elapsed = performance.now() - started;

    expect(view.totalHeight).toBe(320_000);
    expect(view.rows.length).toBeLessThanOrEqual(26);
    expect(view.rows[0]?.offset).toBe(view.startIndex * 32);
    expect(view.rows[0]?.job.filename).toMatch(/^download-/u);
    expect(elapsed).toBeLessThan(250);
  });
});
