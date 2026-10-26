import { describe, expect, it } from "vitest";

import { DesktopShell } from "../src/desktop-shell.js";
import type { EngineLauncher, EngineTransport } from "../src/engine-client.js";
import type { UiRequestEnvelope } from "../src/protocol.js";

class FakeTransport implements EngineTransport {
  readonly sent: UiRequestEnvelope[] = [];
  closeCount = 0;
  #listener: ((message: unknown) => void) | null = null;

  public constructor(private readonly invalidResponse = false) {}

  public subscribe(listener: (message: unknown) => void): () => void {
    this.#listener = listener;
    return () => {
      this.#listener = null;
    };
  }

  public send(message: UiRequestEnvelope): void {
    this.sent.push(message);
    this.#listener?.(
      this.invalidResponse
        ? { unexpected: true }
        : {
            version: 1,
            request_id: message.request_id,
            payload: {
              type: "response",
              response: { type: "health", engine_version: "0.1.0" },
            },
          },
    );
  }

  public close(): void {
    this.closeCount += 1;
  }
}

function launcherFor(
  transport: EngineTransport,
): EngineLauncher & { launches: number } {
  return {
    launches: 0,
    async launch() {
      this.launches += 1;
      return transport;
    },
  };
}

describe("desktop shell engine lifecycle", () => {
  it("launches the engine and completes a typed health handshake", async () => {
    const transport = new FakeTransport();
    const launcher = launcherFor(transport);
    const shell = new DesktopShell(launcher);

    await expect(shell.start()).resolves.toBe("0.1.0");
    expect(shell.status).toBe("running");
    expect(launcher.launches).toBe(1);
    expect(transport.sent[0]?.payload).toEqual({ type: "health" });

    await shell.close();
    await shell.close();
    expect(shell.status).toBe("stopped");
    expect(transport.closeCount).toBe(1);
  });

  it("rejects an invalid engine message and closes the launched process", async () => {
    const transport = new FakeTransport(true);
    const shell = new DesktopShell(launcherFor(transport));

    await expect(shell.start()).rejects.toThrow(
      "invalid engine message envelope",
    );
    expect(shell.status).toBe("stopped");
    expect(transport.closeCount).toBe(1);
  });

  it("returns to stopped when the engine cannot be launched", async () => {
    const shell = new DesktopShell({
      launch: async () => {
        throw new Error("engine executable missing");
      },
    });
    await expect(shell.start()).rejects.toThrow("engine executable missing");
    expect(shell.status).toBe("stopped");
  });
});
