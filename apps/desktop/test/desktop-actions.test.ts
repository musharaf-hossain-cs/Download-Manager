import { describe, expect, it } from "vitest";

import {
  DesktopActionController,
  type DesktopActionPort,
  type DropTargetSnapshot,
} from "../src/desktop-actions.js";
import { jobFixture } from "./fixtures.js";

describe("floating drop target and shell actions", () => {
  it("toggles, constrains, and persists the floating target across reopen", async () => {
    let persisted: DropTargetSnapshot | undefined;
    const port = createPort((snapshot) => {
      persisted = snapshot;
    });
    const controller = new DesktopActionController({}, port);

    await controller.setDropTargetEnabled(true);
    await controller.moveDropTarget(
      { x: 2_000, y: -50 },
      { x: 100, y: 200, width: 800, height: 600 },
    );

    expect(persisted).toEqual({ enabled: true, position: { x: 900, y: 200 } });
    expect(
      new DesktopActionController(persisted ?? {}, port).dropTarget,
    ).toEqual(persisted);
  });

  it("accepts normalized drops only while the target is enabled", () => {
    const disabled = new DesktopActionController({}, createPort());
    expect(() =>
      disabled.acceptDrop([{ kind: "url", value: "https://example.test/a" }]),
    ).toThrow("disabled");

    const enabled = new DesktopActionController(
      { enabled: true },
      createPort(),
    );
    expect(
      enabled.acceptDrop([
        { kind: "text", value: "https://example.test/a\nnot a url" },
      ]),
    ).toMatchObject({
      urls: ["https://example.test/a"],
      invalid: [{ inputIndex: 0, itemIndex: 1, reason: "invalid_url" }],
    });
  });

  it("opens the exact completed file and containing folder paths", async () => {
    const opened: string[] = [];
    const controller = new DesktopActionController(
      {},
      createPort(undefined, opened),
    );
    const job = jobFixture({
      filename: "বাংলা archive.zip",
      destination: "C:\\Downloads\\",
      state: "completed",
    });

    await controller.openJobFile(job);
    await controller.openJobFolder(job);

    expect(opened).toEqual([
      "file:C:\\Downloads\\বাংলা archive.zip",
      "folder:C:\\Downloads",
    ]);
    await expect(
      controller.openJobFile(jobFixture({ state: "paused" })),
    ).rejects.toThrow("only completed");
  });

  it("rolls back the target preference when persistence fails", async () => {
    const controller = new DesktopActionController(
      { enabled: false, position: { x: 10, y: 20 } },
      createPort(() => {
        throw new Error("settings unavailable");
      }),
    );

    await expect(controller.setDropTargetEnabled(true)).rejects.toThrow(
      "settings unavailable",
    );
    expect(controller.dropTarget).toEqual({
      enabled: false,
      position: { x: 10, y: 20 },
    });
  });
});

function createPort(
  save: ((snapshot: DropTargetSnapshot) => void) | undefined = undefined,
  opened: string[] = [],
): DesktopActionPort {
  return {
    saveDropTarget: async (snapshot) => save?.(snapshot),
    openFile: async (path) => {
      opened.push(`file:${path}`);
    },
    openFolder: async (path) => {
      opened.push(`folder:${path}`);
    },
  };
}
