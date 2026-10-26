import { describe, expect, it, vi } from "vitest";

import { AddUrlDialogController } from "../src/add-url-dialog.js";
import {
  type AddJobOptions,
  EngineRejectedError,
} from "../src/engine-client.js";

describe("add URL dialog", () => {
  it("probes a valid URL and preserves Unicode preview fields", async () => {
    const probeUrl = vi.fn(async () => ({
      final_url: "https://example.test/%E0%A6%AB%E0%A6%BE%E0%A6%87%E0%A6%B2",
      filename: "বাংলা.pdf",
      destination: "C:\\ডাউনলোড\\বাংলা.pdf",
      total_bytes: 42,
      mime_type: "application/pdf",
    }));
    const controller = new AddUrlDialogController({
      probeUrl,
      addJob: vi.fn(),
    });
    controller.setUrl("  https://example.test/document  ");
    controller.setDestination("  C:\\ডাউনলোড  ");

    await expect(controller.probe()).resolves.toBe(true);
    expect(probeUrl).toHaveBeenCalledWith(
      "https://example.test/document",
      "C:\\ডাউনলোড",
    );
    expect(controller.state.status).toBe("ready");
    expect(controller.state.preview?.filename).toBe("বাংলা.pdf");
  });

  it("rejects invalid URLs without contacting the engine", async () => {
    const probeUrl = vi.fn();
    const controller = new AddUrlDialogController({
      probeUrl,
      addJob: vi.fn(),
    });
    controller.setUrl("file:///private.txt");
    controller.setDestination("C:\\Downloads");

    await expect(controller.probe()).resolves.toBe(false);
    expect(controller.state.status).toBe("invalid");
    expect(controller.state.errors.url).toBe("Enter an HTTP or HTTPS URL");
    expect(probeUrl).not.toHaveBeenCalled();
  });

  it("shows the engine rejection for an inaccessible destination", async () => {
    const controller = new AddUrlDialogController({
      addJob: vi.fn(),
      probeUrl: async () => {
        throw new EngineRejectedError(
          "destination_unavailable",
          "destination is not writable",
        );
      },
    });
    controller.setUrl("https://example.test/archive.zip");
    controller.setDestination("Z:\\Unavailable");

    await expect(controller.probe()).resolves.toBe(false);
    expect(controller.state).toMatchObject({
      status: "error",
      message: "destination is not writable",
      preview: null,
    });
  });

  it("maps start timing and every duplicate choice to one add request", async () => {
    const addJob = vi.fn<(options: AddJobOptions) => Promise<string>>();
    addJob.mockResolvedValue("job-id");
    const controller = new AddUrlDialogController({
      addJob,
      probeUrl: async () => ({
        final_url: "https://cdn.example.test/archive.zip",
        filename: "archive.zip",
        destination: "C:\\Downloads\\archive.zip",
        total_bytes: 1_024,
        mime_type: "application/zip",
      }),
    });
    controller.setUrl("https://example.test/archive.zip");
    controller.setDestination("C:\\Downloads");
    await controller.probe();

    await controller.submit("now", "focus_existing");
    await controller.submit("later", "add_duplicate");
    await controller.submit("now", "replace_existing");

    expect(addJob).toHaveBeenCalledTimes(3);
    expect(addJob.mock.calls.map(([options]) => options)).toMatchObject([
      { startNow: true, duplicateAction: "focus_existing" },
      { startNow: false, duplicateAction: "add_duplicate" },
      { startNow: true, duplicateAction: "replace_existing" },
    ]);
  });
});
