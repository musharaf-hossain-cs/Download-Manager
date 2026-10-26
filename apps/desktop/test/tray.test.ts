import { describe, expect, it } from "vitest";

import {
  TrayController,
  type CloseBehavior,
  type TrayPort,
} from "../src/tray.js";

describe("system tray and close behavior", () => {
  it("routes every tray command through the desktop port", async () => {
    const calls: string[] = [];
    const controller = new TrayController(
      recordingPort(calls),
      memoryStore(),
      "ask",
    );

    await controller.run({ type: "show" });
    await controller.run({ type: "hide" });
    await controller.run({ type: "quick_add" });
    await controller.run({ type: "set_speed_limit", bytesPerSecond: 1024 });
    await controller.run({ type: "pause_all" });
    await controller.run({ type: "resume_all" });
    await controller.run({ type: "start_queue", queueId: " nightly " });
    await controller.run({ type: "stop_queue", queueId: "nightly" });
    await controller.run({ type: "exit" });

    expect(calls).toEqual([
      "show",
      "hide",
      "quick-add",
      "limit:1024",
      "pause-all",
      "resume-all",
      "start:nightly",
      "stop:nightly",
      "exit",
    ]);
  });

  it("applies minimize, exit, and ask close modes exactly", async () => {
    const calls: string[] = [];
    const store = memoryStore();
    const controller = new TrayController(
      recordingPort(calls),
      store,
      "invalid",
    );

    expect(controller.closeBehavior).toBe("ask");
    await expect(controller.requestClose()).resolves.toBe("cancelled");
    await expect(
      controller.requestClose(async () => "minimize_to_tray"),
    ).resolves.toBe("hidden");
    await controller.setCloseBehavior("exit");
    await expect(controller.requestClose()).resolves.toBe("exited");
    expect(store.saved).toEqual(["exit"]);
    expect(calls).toEqual(["hide", "exit"]);
  });

  it("rolls the close mode back when persistence fails", async () => {
    const controller = new TrayController(
      recordingPort([]),
      {
        save: async () => {
          throw new Error("settings busy");
        },
      },
      "minimize_to_tray",
    );

    await expect(controller.setCloseBehavior("exit")).rejects.toThrow(
      "settings busy",
    );
    expect(controller.closeBehavior).toBe("minimize_to_tray");
  });

  it("rejects invalid speed limits and queue identifiers", async () => {
    const controller = new TrayController(
      recordingPort([]),
      memoryStore(),
      "ask",
    );
    await expect(
      controller.run({ type: "set_speed_limit", bytesPerSecond: 0 }),
    ).rejects.toThrow(RangeError);
    await expect(
      controller.run({ type: "start_queue", queueId: " " }),
    ).rejects.toThrow(RangeError);
  });
});

function memoryStore() {
  const saved: CloseBehavior[] = [];
  return {
    saved,
    save: async (behavior: CloseBehavior) => void saved.push(behavior),
  };
}

function recordingPort(calls: string[]): TrayPort {
  return {
    showWindow: async () => void calls.push("show"),
    hideWindow: async () => void calls.push("hide"),
    quickAdd: async () => void calls.push("quick-add"),
    setSpeedLimit: async (value) => void calls.push(`limit:${String(value)}`),
    pauseAll: async () => void calls.push("pause-all"),
    resumeAll: async () => void calls.push("resume-all"),
    startQueue: async (id) => void calls.push(`start:${id}`),
    stopQueue: async (id) => void calls.push(`stop:${id}`),
    exit: async () => void calls.push("exit"),
  };
}
