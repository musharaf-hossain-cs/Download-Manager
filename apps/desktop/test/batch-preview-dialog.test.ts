import { describe, expect, it, vi } from "vitest";

import {
  BatchPreviewDialogController,
  type BatchJobPort,
  type BatchPreviewInput,
} from "../src/batch-preview-dialog.js";

function input(
  id: string,
  overrides: Partial<BatchPreviewInput> = {},
): BatchPreviewInput {
  return {
    id,
    url: `https://downloads.example.test/${id}.zip`,
    filename: `${id}.zip`,
    mimeType: "application/zip",
    estimatedBytes: 1_024,
    sourcePage: "https://www.example.test/releases",
    ...overrides,
  };
}

function portFixture(): {
  readonly port: BatchJobPort;
  readonly queueJobs: ReturnType<typeof vi.fn<BatchJobPort["queueJobs"]>>;
} {
  const queueJobs = vi.fn<BatchJobPort["queueJobs"]>();
  queueJobs.mockResolvedValue(["job-1"]);
  return { port: { queueJobs }, queueJobs };
}

describe("native batch preview dialog", () => {
  it("previews URL, filename/type, estimated size, and source page", () => {
    const { port } = portFixture();
    const controller = new BatchPreviewDialogController(
      [
        input("manual", {
          url: " HTTPS://Example.TEST:443/files/manual.pdf#page=2 ",
          filename: " বাংলা manual.pdf ",
          mimeType: "Application/PDF; charset=binary",
          estimatedBytes: 42,
          sourcePage: "https://example.test/catalog#documents",
        }),
      ],
      port,
    );

    expect(controller.state.rows).toEqual([
      {
        id: "manual",
        url: "https://example.test/files/manual.pdf",
        filename: "বাংলা manual.pdf",
        mimeType: "application/pdf",
        estimatedBytes: 42,
        sourcePage: "https://example.test/catalog",
        checked: true,
        errors: { url: null, filename: null },
      },
    ]);
  });

  it("searches fields and filters by normalized MIME type", () => {
    const { port } = portFixture();
    const controller = new BatchPreviewDialogController(
      [
        input("archive"),
        input("guide", {
          url: "https://cdn.example.test/docs/guide.pdf",
          filename: "Guide.pdf",
          mimeType: "APPLICATION/PDF",
          sourcePage: "https://learn.example.test/start",
        }),
        input("video", {
          filename: "intro.mp4",
          mimeType: "video/mp4",
        }),
      ],
      port,
    );

    controller.setSearch("learn.example");
    expect(controller.visibleRows.map(({ id }) => id)).toEqual(["guide"]);
    controller.setSearch("");
    controller.setMimeTypeFilter(" application/pdf; charset=utf-8 ");
    expect(controller.visibleRows.map(({ id }) => id)).toEqual(["guide"]);
  });

  it("supports visible selection, URL/filename editing, and queue choice", async () => {
    const { port, queueJobs } = portFixture();
    const controller = new BatchPreviewDialogController(
      [
        input("archive"),
        input("guide", {
          url: "file:///private/guide.pdf",
          filename: "bad\nname.pdf",
          mimeType: "application/pdf",
        }),
        input("video", { mimeType: "video/mp4" }),
      ],
      port,
    );

    expect(controller.state.rows.map(({ checked }) => checked)).toEqual([
      true,
      false,
      true,
    ]);
    controller.setMimeTypeFilter("application/zip");
    controller.setVisibleChecked(false);
    controller.editRow("guide", {
      url: " HTTPS://CDN.example.test:443/docs/guide.pdf#top ",
      filename: " Guide v2.pdf ",
    });
    controller.setChecked("guide", true);
    controller.setChecked("video", false);
    controller.setQueueName(" Research ");

    await expect(controller.queueChecked()).resolves.toEqual(["job-1"]);
    expect(queueJobs).toHaveBeenCalledOnce();
    expect(queueJobs).toHaveBeenCalledWith([
      {
        url: "https://cdn.example.test/docs/guide.pdf",
        filename: "Guide v2.pdf",
        mimeType: "application/pdf",
        estimatedBytes: 1_024,
        sourcePage: "https://www.example.test/releases",
        queueName: "Research",
      },
    ]);
  });

  it("never queues unchecked or invalid URLs", async () => {
    const { port, queueJobs } = portFixture();
    const controller = new BatchPreviewDialogController(
      [
        input("checked", {
          url: "https://example.test:443/checked.zip#mirror",
        }),
        input("unchecked"),
        input("invalid", { url: "javascript:alert(1)" }),
      ],
      port,
    );
    controller.setChecked("unchecked", false);

    await controller.queueChecked();
    expect(queueJobs).toHaveBeenCalledWith([
      expect.objectContaining({
        url: "https://example.test/checked.zip",
      }),
    ]);
    expect(queueJobs.mock.calls[0]?.[0]).toHaveLength(1);
    expect(() => controller.setChecked("invalid", true)).toThrow(
      "invalid rows cannot be checked",
    );
  });

  it("refuses an empty effective selection and invalid batch boundaries", async () => {
    const { port, queueJobs } = portFixture();
    const controller = new BatchPreviewDialogController(
      [input("only", { url: "ftp://example.test/file" })],
      port,
    );

    await expect(controller.queueChecked()).rejects.toThrow(
      "select at least one valid download",
    );
    expect(queueJobs).not.toHaveBeenCalled();
    expect(() => new BatchPreviewDialogController([], port)).toThrow(
      "between 1 and 128",
    );
    expect(
      () =>
        new BatchPreviewDialogController([input("same"), input("same")], port),
    ).toThrow("unique");
  });
});
