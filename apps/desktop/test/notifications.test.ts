import { describe, expect, it } from "vitest";

import {
  WindowsNotificationController,
  type WindowsNotificationPort,
  type WindowsToast,
} from "../src/notifications.js";

describe("Windows completion and failure notifications", () => {
  it("sends deterministic completion and failure toasts through a mock port", async () => {
    const port = recordingPort();
    const controller = new WindowsNotificationController(port, () => true);

    await expect(
      controller.notify({
        type: "completed",
        jobId: "job-a",
        filename: "archive.zip",
        path: "C:\\Downloads\\archive.zip",
      }),
    ).resolves.toBe("shown");
    await expect(
      controller.notify({
        type: "failed",
        jobId: "job-b",
        filename: "video.mp4",
        reason: "The server closed the connection",
      }),
    ).resolves.toBe("shown");

    expect(port.toasts).toEqual([
      {
        title: "Download complete",
        body: "archive.zip was saved to C:\\Downloads\\archive.zip",
        tag: "download:job-a",
        group: "downloads",
        severity: "success",
      },
      {
        title: "Download failed",
        body: "video.mp4: The server closed the connection",
        tag: "download:job-b",
        group: "downloads",
        severity: "error",
      },
    ]);
  });

  it("does not call the OS port when disabled or unsupported", async () => {
    const disabled = recordingPort();
    const unsupported = recordingPort(false);
    const event = {
      type: "completed" as const,
      jobId: "job-a",
      filename: "archive.zip",
      path: "C:\\Downloads\\archive.zip",
    };

    await expect(
      new WindowsNotificationController(disabled, () => false).notify(event),
    ).resolves.toBe("disabled");
    await expect(
      new WindowsNotificationController(unsupported, () => true).notify(event),
    ).resolves.toBe("unsupported");
    expect(disabled.toasts).toEqual([]);
    expect(unsupported.toasts).toEqual([]);
  });

  it("exercises the safe real-adapter smoke payload", async () => {
    const port = recordingPort();

    await expect(
      new WindowsNotificationController(port, () => true).smoke(),
    ).resolves.toBe("shown");
    expect(port.toasts).toEqual([
      {
        title: "Download Manager notifications are ready",
        body: "Completion and failure notifications can appear here.",
        tag: "download-manager:notification-smoke",
        group: "downloads",
        severity: "info",
      },
    ]);
  });

  it("rejects blank notification details before showing a toast", async () => {
    const port = recordingPort();
    const controller = new WindowsNotificationController(port, () => true);

    await expect(
      controller.notify({
        type: "failed",
        jobId: "job-a",
        filename: " ",
        reason: "network error",
      }),
    ).rejects.toThrow(RangeError);
    expect(port.toasts).toEqual([]);
  });
});

function recordingPort(supported = true): WindowsNotificationPort & {
  readonly toasts: WindowsToast[];
} {
  const toasts: WindowsToast[] = [];
  return {
    toasts,
    isSupported: async () => supported,
    show: async (toast) => void toasts.push(toast),
  };
}
