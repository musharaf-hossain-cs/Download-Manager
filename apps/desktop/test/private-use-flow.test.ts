import { describe, expect, it, vi } from "vitest";

import { AddUrlDialogController } from "../src/add-url-dialog.js";
import { DownloadListController } from "../src/download-list.js";
import { EngineClient } from "../src/engine-client.js";
import { JOB_POLL_INTERVAL_MS } from "../src/private-use-app.js";
import { TauriEngineTransport } from "../src/tauri-transport.js";
import { jobFixture } from "./fixtures.js";

describe("private-use desktop flow", () => {
  it("carries probe, add, and list through the typed Tauri/backend bridge", async () => {
    const job = jobFixture({
      id: "job-from-backend",
      source_url: "https://example.test/archive.zip",
      filename: "archive.zip",
      destination: "C:\\Downloads",
      state: "downloading",
      downloaded_bytes: 256,
      total_bytes: 1_024,
    });
    const invoke = vi.fn(async (command: string, arguments_: unknown) => {
      expect(command).toBe("backend_request");
      const request = JSON.parse(
        (arguments_ as { request: string }).request,
      ) as {
        version: number;
        request_id: string;
        payload: { type: string };
      };
      let response: unknown;
      switch (request.payload.type) {
        case "probe_url":
          response = {
            type: "url_probe",
            final_url: "https://cdn.example.test/archive.zip",
            filename: "archive.zip",
            destination: "C:\\Downloads\\archive.zip",
            total_bytes: 1_024,
            mime_type: "application/zip",
          };
          break;
        case "add_job":
          response = { type: "accepted", job_id: job.id };
          break;
        case "list_jobs":
          response = { type: "jobs", jobs: [job] };
          break;
        default:
          throw new Error(`unexpected request ${request.payload.type}`);
      }
      return JSON.stringify({
        version: request.version,
        request_id: request.request_id,
        payload: { type: "response", response },
      });
    });
    let nextId = 0;
    const engine = new EngineClient(
      new TauriEngineTransport(invoke),
      () => `request-${String(++nextId)}`,
    );
    const add = new AddUrlDialogController(engine);
    add.setUrl("https://example.test/archive.zip");
    add.setDestination("C:\\Downloads");

    await expect(add.probe()).resolves.toBe(true);
    expect(add.state.preview?.filename).toBe("archive.zip");
    await expect(add.submit("now", "add_duplicate")).resolves.toBe(job.id);

    const list = new DownloadListController(engine);
    await list.load();
    expect(list.state.jobs).toEqual([job]);
    expect(invoke).toHaveBeenCalledTimes(3);
    await engine.close();
  });

  it("uses a bounded polling interval instead of packet-rate rendering", () => {
    expect(JOB_POLL_INTERVAL_MS).toBe(500);
    expect(JOB_POLL_INTERVAL_MS).toBeGreaterThanOrEqual(250);
  });

  it("carries pause, resume, cancel, and removal through the production bridge", async () => {
    const requests: string[] = [];
    const invoke = vi.fn(async (_command: string, arguments_: unknown) => {
      const request = JSON.parse(
        (arguments_ as { request: string }).request,
      ) as {
        version: number;
        request_id: string;
        payload: {
          type: string;
          job_id: string;
          action: string;
        };
      };
      requests.push(`${request.payload.type}:${request.payload.action}`);
      const response =
        request.payload.type === "destructive_job"
          ? {
              type: "job_removed",
              job_id: request.payload.job_id,
              file_deleted: false,
            }
          : {
              type: "job_state",
              job_id: request.payload.job_id,
              state:
                request.payload.action === "pause"
                  ? "paused"
                  : request.payload.action === "resume"
                    ? "queued"
                    : "cancelled",
              updated_at: 1_700_000_000_000,
            };
      return JSON.stringify({
        version: request.version,
        request_id: request.request_id,
        payload: { type: "response", response },
      });
    });
    let nextId = 0;
    const engine = new EngineClient(
      new TauriEngineTransport(invoke),
      () => `control-${String(++nextId)}`,
    );
    const jobId = "018f47ce-8e51-7f31-a1ec-4a97f84a12cd";

    await engine.controlJob(jobId, "pause");
    await engine.controlJob(jobId, "resume");
    await engine.controlJob(jobId, "cancel");
    await engine.removeJob(jobId, "remove_record", null);

    expect(requests).toEqual([
      "control_job:pause",
      "control_job:resume",
      "control_job:cancel",
      "destructive_job:remove_record",
    ]);
    await engine.close();
  });
});
