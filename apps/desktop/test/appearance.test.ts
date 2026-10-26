import { describe, expect, it } from "vitest";

import {
  AppearanceController,
  DOWNLOAD_COLUMN_IDS,
  type AppearanceSnapshot,
} from "../src/appearance.js";

describe("appearance preferences", () => {
  it("resolves light, dark, and system themes", async () => {
    let saved: AppearanceSnapshot | undefined;
    const controller = new AppearanceController(
      {},
      {
        save: async (snapshot) => {
          saved = snapshot;
        },
      },
    );

    expect(controller.resolveTheme(false)).toBe("light");
    expect(controller.resolveTheme(true)).toBe("dark");
    await controller.setTheme("dark");
    expect(controller.resolveTheme(false)).toBe("dark");
    expect(saved?.theme).toBe("dark");
  });

  it("reopens with the persisted theme, column order, and visibility", async () => {
    let persisted: AppearanceSnapshot = {
      theme: "system",
      columns: DOWNLOAD_COLUMN_IDS.map((id) => ({ id, visible: true })),
    };
    const store = {
      save: async (snapshot: AppearanceSnapshot) => {
        persisted = snapshot;
      },
    };
    const first = new AppearanceController(persisted, store);
    await first.setTheme("light");
    await first.setColumnVisible("description", false);
    await first.moveColumn("progress", "status");

    const reopened = new AppearanceController(persisted, store);
    expect(reopened.snapshot).toMatchInlineSnapshot(`
      {
        "columns": [
          {
            "id": "name",
            "visible": true,
          },
          {
            "id": "size",
            "visible": true,
          },
          {
            "id": "progress",
            "visible": true,
          },
          {
            "id": "status",
            "visible": true,
          },
          {
            "id": "speed",
            "visible": true,
          },
          {
            "id": "eta",
            "visible": true,
          },
          {
            "id": "transferred",
            "visible": true,
          },
          {
            "id": "date_added",
            "visible": true,
          },
          {
            "id": "date_completed",
            "visible": true,
          },
          {
            "id": "url_host",
            "visible": true,
          },
          {
            "id": "category",
            "visible": true,
          },
          {
            "id": "queue",
            "visible": true,
          },
          {
            "id": "description",
            "visible": false,
          },
        ],
        "theme": "light",
      }
    `);
  });

  it("fills new columns and rolls back a rejected persistence update", async () => {
    const controller = new AppearanceController(
      { theme: "dark", columns: [{ id: "name", visible: false }] },
      { save: async () => Promise.reject(new Error("settings unavailable")) },
    );

    expect(controller.snapshot.columns).toHaveLength(
      DOWNLOAD_COLUMN_IDS.length,
    );
    await expect(controller.setTheme("light")).rejects.toThrow(
      "settings unavailable",
    );
    expect(controller.snapshot.theme).toBe("dark");
  });
});
