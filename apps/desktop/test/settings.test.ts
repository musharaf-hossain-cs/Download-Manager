import { describe, expect, it } from "vitest";

import {
  SettingsController,
  type SettingDefinition,
  type SettingsSnapshot,
} from "../src/settings.js";

const definitions: readonly SettingDefinition[] = [
  {
    section: "general",
    key: "closeBehavior",
    label: "When closing the window",
    description: "Choose whether the app exits or stays available in the tray.",
    keywords: ["exit", "tray"],
    defaultValue: "tray",
  },
  {
    section: "downloads",
    key: "concurrentDownloads",
    label: "Concurrent downloads",
    description: "Maximum number of downloads running at once.",
    keywords: ["parallel", "limit"],
    defaultValue: 4,
  },
  {
    section: "downloads",
    key: "showCompletionNotification",
    label: "Completion notification",
    description: "Show a notification after a download completes.",
    defaultValue: true,
  },
];

describe("searchable settings", () => {
  it("finds settings by label, description, and keywords", () => {
    const controller = createController({}, async () => undefined);
    expect(controller.search("exit").map((result) => result.key)).toEqual([
      "closeBehavior",
    ]);
    expect(
      controller.search("maximum downloads").map((result) => result.key),
    ).toEqual(["concurrentDownloads"]);
    expect(
      controller.search("notification").map((result) => result.key),
    ).toEqual(["showCompletionNotification"]);
  });

  it("resets one section without changing other settings, jobs, or files", async () => {
    const jobs = [{ id: "job-a" }];
    const files = new Set(["C:\\Downloads\\archive.zip"]);
    const saved: SettingsSnapshot[] = [];
    const controller = createController(
      {
        general: { closeBehavior: "exit" },
        downloads: {
          concurrentDownloads: 12,
          showCompletionNotification: false,
        },
      },
      async (settings) => {
        saved.push(settings);
      },
    );

    await controller.resetSection("downloads");

    expect(controller.values).toEqual({
      general: { closeBehavior: "exit" },
      downloads: { concurrentDownloads: 4, showCompletionNotification: true },
    });
    expect(saved).toHaveLength(1);
    expect(jobs).toEqual([{ id: "job-a" }]);
    expect(files).toEqual(new Set(["C:\\Downloads\\archive.zip"]));
  });

  it("resets all preference sections and preserves job/file state", async () => {
    const jobs = new Map([["job-a", "completed"]]);
    const files = new Set(["archive.zip"]);
    const controller = createController(
      {
        general: { closeBehavior: "exit" },
        downloads: {
          concurrentDownloads: 8,
          showCompletionNotification: false,
        },
      },
      async () => undefined,
    );

    await controller.resetAll();

    expect(controller.values).toEqual({
      general: { closeBehavior: "tray" },
      downloads: { concurrentDownloads: 4, showCompletionNotification: true },
    });
    expect(jobs).toEqual(new Map([["job-a", "completed"]]));
    expect(files).toEqual(new Set(["archive.zip"]));
  });

  it("restores local state when persistence rejects an update", async () => {
    const controller = createController({}, async () => {
      throw new Error("settings database busy");
    });
    await expect(
      controller.setValue("downloads", "concurrentDownloads", 6),
    ).rejects.toThrow("settings database busy");
    expect(controller.values.downloads?.concurrentDownloads).toBe(4);
  });
});

function createController(
  persisted: SettingsSnapshot,
  save: (settings: SettingsSnapshot) => Promise<void>,
): SettingsController {
  return new SettingsController(definitions, persisted, { save });
}
