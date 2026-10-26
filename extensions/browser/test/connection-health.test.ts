import { describe, expect, it } from "vitest";

import {
  EXTENSION_PROTOCOL_VERSION,
  NATIVE_HOST_NAME,
  NativeConnectionHealth,
  type NativeHealthBridge,
  type NativeHealthPort,
} from "../src/index.js";

class FakePort implements NativeHealthPort {
  public posted: unknown[] = [];
  #message: ((message: unknown) => void) | null = null;
  #disconnect: (() => void) | null = null;

  public postMessage(message: unknown): void {
    this.posted.push(message);
  }

  public onMessage(listener: (message: unknown) => void): void {
    this.#message = listener;
  }

  public onDisconnect(listener: () => void): void {
    this.#disconnect = listener;
  }

  public respond(message: unknown): void {
    this.#message?.(message);
  }

  public disconnect(): void {
    this.#disconnect?.();
  }
}

function fixture() {
  const port = new FakePort();
  const badges: Array<{ text: string; title: string }> = [];
  const diagnostics: unknown[] = [];
  let connectedHost = "";
  const bridge: NativeHealthBridge = {
    connectNative(hostName) {
      connectedHost = hostName;
      return port;
    },
    setBadge(badge) {
      badges.push(badge);
    },
    openDiagnostics(diagnostic) {
      diagnostics.push(diagnostic);
    },
  };
  const health = new NativeConnectionHealth(bridge, "0.1.0");
  health.connect({
    requestId: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
    nonce: "abcdefghijklmnop",
    extensionId: "abcdefghijklmnopabcdefghijklmnop",
  });
  return { badges, connectedHost, diagnostics, health, port };
}

describe("native connection health", () => {
  it("shows a connected badge only for a correlated compatible response", () => {
    const { badges, connectedHost, health, port } = fixture();
    expect(connectedHost).toBe(NATIVE_HOST_NAME);
    expect(port.posted).toEqual([
      expect.objectContaining({ payload: { type: "health" } }),
    ]);

    port.respond({
      version: EXTENSION_PROTOCOL_VERSION,
      request_id: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
      payload: { type: "healthy", host_version: "0.1.0" },
    });

    expect(health.state).toBe("connected");
    expect(badges.at(-1)).toMatchObject({
      text: "✓",
      title: expect.stringContaining("connected"),
    });
  });

  it("shows a disconnected error when the native port closes", () => {
    const { badges, health, port } = fixture();
    port.disconnect();
    expect(health.state).toBe("disconnected");
    expect(badges.at(-1)).toMatchObject({
      text: "!",
      title: expect.stringContaining("disconnected"),
    });
  });

  it("distinguishes host and protocol version mismatches", () => {
    for (const response of [
      {
        version: EXTENSION_PROTOCOL_VERSION,
        request_id: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
        payload: { type: "healthy", host_version: "9.0.0" },
      },
      {
        version: 2,
        request_id: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
        payload: { type: "healthy", host_version: "0.1.0" },
      },
    ]) {
      const { badges, health, port } = fixture();
      port.respond(response);
      expect(health.state).toBe("version_mismatch");
      expect(badges.at(-1)).toMatchObject({
        text: "↕",
        title: expect.stringContaining("mismatch"),
      });
    }
  });

  it("opens diagnostics with the exact version mismatch and repair action", () => {
    const { diagnostics, health, port } = fixture();
    port.respond({
      version: EXTENSION_PROTOCOL_VERSION,
      request_id: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
      payload: { type: "healthy", host_version: "9.0.0" },
    });

    health.openDiagnostics();

    expect(diagnostics).toEqual([
      {
        code: "host_version_mismatch",
        expectedProtocolVersion: 1,
        actualProtocolVersion: 1,
        expectedHostVersion: "0.1.0",
        actualHostVersion: "9.0.0",
        recommendedAction: expect.stringContaining("reinstall"),
      },
    ]);
  });
});
