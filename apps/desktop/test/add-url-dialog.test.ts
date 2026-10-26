import { describe, expect, it, vi } from "vitest";

import { AddUrlDialogController } from "../src/add-url-dialog.js";
import { EngineRejectedError } from "../src/engine-client.js";

describe("add URL dialog", () => {
  it("probes a valid URL and preserves Unicode preview fields", async () => {
    const probeUrl = vi.fn(async () => ({
      final_url: "https://example.test/%E0%A6%AB%E0%A6%BE%E0%A6%87%E0%A6%B2",
      filename: "বাংলা.pdf",
      destination: "C:\\ডাউনলোড\\বাংলা.pdf",
      total_bytes: 42,
      mime_type: "application/pdf",
    }));
    const controller = new AddUrlDialogController({ probeUrl });
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
    const controller = new AddUrlDialogController({ probeUrl });
    controller.setUrl("file:///private.txt");
    controller.setDestination("C:\\Downloads");

    await expect(controller.probe()).resolves.toBe(false);
    expect(controller.state.status).toBe("invalid");
    expect(controller.state.errors.url).toBe("Enter an HTTP or HTTPS URL");
    expect(probeUrl).not.toHaveBeenCalled();
  });

  it("shows the engine rejection for an inaccessible destination", async () => {
    const controller = new AddUrlDialogController({
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
});
