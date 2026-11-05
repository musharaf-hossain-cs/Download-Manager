import { describe, expect, it, vi } from "vitest";

import { PrivateUseDesktopServices } from "../src/private-use-services.js";

describe("private-use native services", () => {
  it("persists the last folder and restores it after reconstruction", async () => {
    let persisted: string | null = null;
    const invoke = vi.fn(async (command: string, arguments_: unknown) => {
      if (command === "load_desktop_preferences") {
        return { last_download_directory: persisted };
      }
      if (command === "save_last_download_directory") {
        persisted = (arguments_ as { directory: string }).directory;
        return null;
      }
      throw new Error(`unexpected command ${command}`);
    });

    const first = new PrivateUseDesktopServices(invoke);
    await first.saveLastDownloadDirectory("C:\\Downloads");
    const reopened = new PrivateUseDesktopServices(invoke);
    await expect(reopened.loadPreferences()).resolves.toEqual({
      last_download_directory: "C:\\Downloads",
    });
  });

  it("passes the same completed path to open and reveal commands", async () => {
    const calls: Array<{ command: string; path: string }> = [];
    const invoke = vi.fn(async (command: string, arguments_: unknown) => {
      calls.push({
        command,
        path: (arguments_ as { path: string }).path,
      });
      return null;
    });
    const services = new PrivateUseDesktopServices(invoke);
    const path = "C:\\Downloads\\বাংলা archive.zip";
    await services.openFile(path);
    await services.revealFile(path);
    expect(calls).toEqual([
      { command: "open_local_file", path },
      { command: "reveal_local_file", path },
    ]);
  });

  it("loads local diagnostics without exposing an upload command", async () => {
    const invoke = vi.fn(async () => ({
      product_version: "0.1.0",
      backend_health: "Healthy (backend 0.1.0)",
      state_path: "C:\\Local\\DownloadManager\\State",
      log_path: "C:\\Local\\DownloadManager\\Logs\\backend.log",
    }));
    const services = new PrivateUseDesktopServices(invoke);
    await expect(services.diagnostics()).resolves.toMatchObject({
      product_version: "0.1.0",
      backend_health: expect.stringContaining("Healthy"),
    });
    expect(invoke).toHaveBeenCalledWith("desktop_diagnostics", {});
  });
});
