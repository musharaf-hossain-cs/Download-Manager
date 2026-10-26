import { describe, expect, it, vi } from "vitest";

import {
  MediaOverlayInteractionController,
  type MediaOverlayPreferencesPort,
  type MediaOverlayPreferencesSnapshot,
} from "../src/media-overlay-interaction.js";

class MemoryPort implements MediaOverlayPreferencesPort {
  readonly saves: MediaOverlayPreferencesSnapshot[] = [];
  fail = false;

  async save(snapshot: MediaOverlayPreferencesSnapshot): Promise<void> {
    if (this.fail) throw new Error("storage unavailable");
    this.saves.push(structuredClone(snapshot));
  }
}

describe("media overlay interaction preferences", () => {
  it("applies exact per-site exceptions before the global enablement", async () => {
    const port = new MemoryPort();
    const controller = new MediaOverlayInteractionController({}, port);
    expect(controller.isEnabled("https://watch.example.test/video")).toBe(true);

    await controller.setGlobalEnabled(false);
    await controller.setSiteEnabled("https://watch.example.test/video", true);
    expect(controller.isEnabled("https://watch.example.test/other")).toBe(true);
    expect(controller.isEnabled("https://cdn.watch.example.test/video")).toBe(
      false,
    );

    await controller.setSiteEnabled("https://watch.example.test/", null);
    expect(controller.isEnabled("https://watch.example.test/video")).toBe(
      false,
    );
    expect(port.saves).toHaveLength(3);
  });

  it("persists minimized and viewport-clamped drag state per site", async () => {
    const port = new MemoryPort();
    const controller = new MediaOverlayInteractionController({}, port);
    await controller.setMinimized("https://watch.example.test/a", true);
    await controller.move(
      "https://watch.example.test/b",
      { x: 900, y: -20 },
      { x: 10, y: 20, width: 630, height: 460 },
    );

    expect(controller.stateFor("https://watch.example.test/elsewhere")).toEqual(
      { minimized: true, position: { x: 640, y: 20 } },
    );
    const reopened = new MediaOverlayInteractionController(
      port.saves.at(-1) ?? {},
      new MemoryPort(),
    );
    expect(reopened.stateFor("https://WATCH.example.test/")).toEqual({
      minimized: true,
      position: { x: 640, y: 20 },
    });
  });

  it("rolls back failed persistence and rejects invalid site or geometry", async () => {
    const port = new MemoryPort();
    const controller = new MediaOverlayInteractionController(
      { globalEnabled: true },
      port,
    );
    port.fail = true;

    await expect(controller.setGlobalEnabled(false)).rejects.toThrow(
      "storage unavailable",
    );
    expect(controller.snapshot.globalEnabled).toBe(true);
    expect(() => controller.isEnabled("file:///private/video.mp4")).toThrow(
      "absolute HTTP(S)",
    );
    await expect(
      controller.move(
        "https://watch.example.test/",
        { x: Number.NaN, y: 0 },
        { x: 0, y: 0, width: 100, height: 100 },
      ),
    ).rejects.toThrow("position must be finite");
  });

  it("normalizes malformed persisted entries without sharing snapshots", () => {
    const source = {
      globalEnabled: false,
      siteEnabled: { "GOOD.example.test": true, "bad..test": false },
      siteState: {
        "good.example.test": {
          minimized: true,
          position: { x: 12, y: 34 },
        },
        "bad..test": { minimized: true, position: { x: 1, y: 2 } },
      },
    };
    const controller = new MediaOverlayInteractionController(source, {
      save: vi.fn(),
    });
    const snapshot = controller.snapshot;
    (snapshot.siteEnabled as Record<string, boolean>)["good.example.test"] =
      false;

    expect(controller.isEnabled("https://good.example.test/")).toBe(true);
    expect(controller.snapshot.siteEnabled).toEqual({
      "good.example.test": true,
    });
    expect(controller.snapshot.siteState).toEqual({
      "good.example.test": {
        minimized: true,
        position: { x: 12, y: 34 },
      },
    });
  });
});
