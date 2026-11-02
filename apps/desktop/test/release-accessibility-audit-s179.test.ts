import { describe, expect, it } from "vitest";

import {
  DesktopAccessibilityModel,
  planScaledDesktopLayout,
  type DesktopAccessibilityInput,
  type WindowsScaleProfile,
} from "../src/accessibility.js";
import {
  BANGLA_MESSAGES,
  createTextExpansionCatalog,
  ENGLISH_MESSAGES,
  type MessageCatalog,
} from "../src/localization.js";

const PRIMARY_TOOLBAR = [
  ["add", "toolbar.addUrl"],
  ["resume", "toolbar.resume"],
  ["stop", "toolbar.stop"],
  ["stop-all", "toolbar.stopAll"],
  ["delete", "toolbar.delete"],
  ["delete-completed", "toolbar.deleteCompleted"],
  ["options", "toolbar.options"],
  ["scheduler", "toolbar.scheduler"],
  ["queue", "toolbar.queue"],
] as const;

function releaseWindow(catalog: MessageCatalog): DesktopAccessibilityInput {
  return {
    windowName: catalog["app.title"],
    toolbarName: catalog["toolbar.name"],
    downloadsName: catalog["downloads.name"],
    toolbar: PRIMARY_TOOLBAR.map(([id, key]) => ({ id, name: catalog[key] })),
    columns: [
      { id: "name", name: catalog["columns.name"], sort: "ascending" },
      { id: "status", name: catalog["columns.status"] },
      { id: "progress", name: catalog["columns.progress"] },
    ],
    rows: [
      {
        id: "বাংলা-ফাইল",
        name: "প্রতিবেদন ২০২৬.pdf",
        stateText: catalog["states.downloading"],
        stateSymbol: "↓",
        cells: {
          name: "প্রতিবেদন ২০২৬.pdf",
          status: catalog["states.downloading"],
          progress: "42 percent",
        },
      },
    ],
  };
}

describe("S179 final desktop accessibility audit", () => {
  it.each([
    ["English", ENGLISH_MESSAGES],
    ["Bangla", BANGLA_MESSAGES],
  ] as const)(
    "passes the automated semantics scan and primary keyboard flow in %s",
    (_locale, catalog) => {
      const model = new DesktopAccessibilityModel(releaseWindow(catalog));

      expect(model.scan()).toEqual([]);
      const reachedToolbarControls = [model.key("Tab").focusedId];
      for (let index = 1; index < PRIMARY_TOOLBAR.length; index += 1) {
        reachedToolbarControls.push(model.key("ArrowRight").focusedId);
      }
      expect(reachedToolbarControls).toEqual(
        PRIMARY_TOOLBAR.map(([id]) => `toolbar.${id}`),
      );
      expect(model.key("Enter")).toEqual({
        focusedId: "toolbar.queue",
        activatedId: "toolbar.queue",
      });
      expect(model.key("Tab").focusedId).toBe("column.name");
      expect(model.key("End").focusedId).toBe("column.progress");
      expect(model.key("Tab").focusedId).toBe("download.বাংলা-ফাইল");
      expect(model.key("Enter").activatedId).toBe("download.বাংলা-ফাইল");
      expect(model.focused?.style).toMatchObject({
        focusOutline: "Highlight",
        focusOutlineWidth: 2,
      });
    },
  );

  it.each([
    ["English", ENGLISH_MESSAGES],
    ["Bangla", BANGLA_MESSAGES],
  ] as const)(
    "keeps expanded %s controls reachable across 100–250%% DPI and monitors",
    (_locale, catalog) => {
      const expanded = createTextExpansionCatalog(catalog);
      const toolbarNames = PRIMARY_TOOLBAR.map(([, key]) => expanded[key]);
      const profiles: readonly WindowsScaleProfile[] = [
        {
          monitorId: "laptop-100",
          workAreaWidthPixels: 1920,
          workAreaHeightPixels: 1080,
          dpiPercent: 100,
          textPercent: 100,
        },
        {
          monitorId: "laptop-125",
          workAreaWidthPixels: 1920,
          workAreaHeightPixels: 1080,
          dpiPercent: 125,
          textPercent: 125,
        },
        {
          monitorId: "laptop-150",
          workAreaWidthPixels: 2560,
          workAreaHeightPixels: 1440,
          dpiPercent: 150,
          textPercent: 150,
        },
        {
          monitorId: "external-175",
          workAreaWidthPixels: 3840,
          workAreaHeightPixels: 2160,
          dpiPercent: 175,
          textPercent: 175,
        },
        {
          monitorId: "external-200",
          workAreaWidthPixels: 3840,
          workAreaHeightPixels: 2160,
          dpiPercent: 200,
          textPercent: 200,
        },
        {
          monitorId: "external-225",
          workAreaWidthPixels: 3840,
          workAreaHeightPixels: 2160,
          dpiPercent: 225,
          textPercent: 225,
        },
        {
          monitorId: "external-250",
          workAreaWidthPixels: 3840,
          workAreaHeightPixels: 2160,
          dpiPercent: 250,
          textPercent: 225,
        },
      ];

      for (const profile of profiles) {
        const layout = planScaledDesktopLayout(profile, toolbarNames);
        expect(layout.minimumControlWidth).toBeGreaterThanOrEqual(44);
        expect(layout.minimumControlHeight).toBeGreaterThanOrEqual(44);
        expect(
          layout.visibleToolbarControls + layout.overflowToolbarControls,
        ).toBe(PRIMARY_TOOLBAR.length);
        expect(layout.toolbarRows).toBeLessThanOrEqual(2);
        expect(layout.downloadsHeight).toBeGreaterThanOrEqual(160);
      }
    },
  );

  it("rejects unsupported scaling and unlabeled toolbar input", () => {
    const base: WindowsScaleProfile = {
      monitorId: "primary",
      workAreaWidthPixels: 1920,
      workAreaHeightPixels: 1080,
      dpiPercent: 100,
      textPercent: 100,
    };

    expect(() =>
      planScaledDesktopLayout({ ...base, dpiPercent: 251 }, ["Add URL"]),
    ).toThrow("DPI scaling must be between 100 and 250 percent");
    expect(() => planScaledDesktopLayout(base, [" "])).toThrow(
      "toolbar controls need accessible names",
    );
  });
});
