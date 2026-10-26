import { describe, expect, it } from "vitest";

import {
  BrowserIntegrationDiagnosticsController,
  diagnoseBrowserIntegration,
  type BrowserIntegrationExpectations,
  type BrowserIntegrationRepairAction,
  type BrowserIntegrationSnapshot,
} from "../src/browser-integration-diagnostics.js";

const expectations: BrowserIntegrationExpectations = {
  appVersion: "0.1.0",
  extensionId: "abcdefghijklmnopabcdefghijklmnop",
  nativeHostName: "io.github.musharaf_hossain_cs.download_manager",
  protocolVersion: 1,
  browsers: ["chrome", "edge"],
};

function healthySnapshot(): BrowserIntegrationSnapshot {
  return {
    appVersion: "0.1.0",
    extension: {
      installed: true,
      id: "abcdefghijklmnopabcdefghijklmnop",
      version: "0.1.0",
    },
    nativeHost: {
      registeredBrowsers: ["chrome", "edge"],
      manifest: {
        name: "io.github.musharaf_hossain_cs.download_manager",
        executablePath: "C:\\Program Files\\Download Manager\\native-host.exe",
        allowedExtensionIds: ["abcdefghijklmnopabcdefghijklmnop"],
      },
      executableExists: true,
      reachable: true,
      protocolVersion: 1,
      version: "0.1.0",
    },
  };
}

function failedIds(snapshot: BrowserIntegrationSnapshot): readonly string[] {
  return diagnoseBrowserIntegration(snapshot, expectations)
    .checks.filter((check) => check.status === "fail")
    .map((check) => check.id);
}

describe("browser integration diagnostics", () => {
  it("reports a healthy app, extension, registration, and native host", () => {
    const report = diagnoseBrowserIntegration(healthySnapshot(), expectations);

    expect(report.status).toBe("healthy");
    expect(report.summary).toBe("Browser integration is healthy.");
    expect(report.checks.every((check) => check.status === "pass")).toBe(true);
    expect(report.repairActions).toEqual([]);
  });

  it("identifies the exact broken fixture and recommended repair", () => {
    const fixtures: readonly [
      string,
      (snapshot: BrowserIntegrationSnapshot) => BrowserIntegrationSnapshot,
      string,
      BrowserIntegrationRepairAction,
    ][] = [
      [
        "extension version",
        (snapshot) => ({
          ...snapshot,
          extension: { ...snapshot.extension, version: "9.0.0" },
        }),
        "extension.version",
        "update_extension",
      ],
      [
        "Chrome registration",
        (snapshot) => ({
          ...snapshot,
          nativeHost: {
            ...snapshot.nativeHost,
            registeredBrowsers: ["edge"],
          },
        }),
        "registration.chrome",
        "repair_registration",
      ],
      [
        "manifest owner",
        (snapshot) => ({
          ...snapshot,
          nativeHost: {
            ...snapshot.nativeHost,
            manifest: {
              ...snapshot.nativeHost.manifest!,
              name: "example.untrusted.host",
            },
          },
        }),
        "native_host.manifest",
        "repair_registration",
      ],
      [
        "host executable",
        (snapshot) => ({
          ...snapshot,
          nativeHost: { ...snapshot.nativeHost, executableExists: false },
        }),
        "native_host.executable",
        "reinstall_application",
      ],
      [
        "manifest allowlist",
        (snapshot) => ({
          ...snapshot,
          nativeHost: {
            ...snapshot.nativeHost,
            manifest: {
              ...snapshot.nativeHost.manifest!,
              allowedExtensionIds: [],
            },
          },
        }),
        "native_host.allowlist",
        "repair_registration",
      ],
      [
        "native protocol",
        (snapshot) => ({
          ...snapshot,
          nativeHost: { ...snapshot.nativeHost, protocolVersion: 2 },
        }),
        "protocol.version",
        "reinstall_application",
      ],
      [
        "native-host version",
        (snapshot) => ({
          ...snapshot,
          nativeHost: { ...snapshot.nativeHost, version: "9.0.0" },
        }),
        "native_host.version",
        "reinstall_application",
      ],
    ];

    for (const [name, breakFixture, expectedId, expectedRepair] of fixtures) {
      const report = diagnoseBrowserIntegration(
        breakFixture(healthySnapshot()),
        expectations,
      );
      expect(failedIds(breakFixture(healthySnapshot())), name).toEqual([
        expectedId,
      ]);
      expect(
        report.checks.find((check) => check.id === expectedId),
        name,
      ).toMatchObject({
        status: "fail",
        repairAction: expectedRepair,
      });
      expect(
        report.checks.find((check) => check.id === expectedId)
          ?.recommendedAction,
        name,
      ).toBeTruthy();
    }
  });

  it("blocks dependent checks when the manifest or connection is unavailable", () => {
    const snapshot = healthySnapshot();
    const report = diagnoseBrowserIntegration(
      {
        ...snapshot,
        nativeHost: {
          ...snapshot.nativeHost,
          manifest: null,
          reachable: false,
          protocolVersion: null,
          version: null,
        },
      },
      expectations,
    );

    expect(
      report.checks
        .filter((check) => check.status === "fail")
        .map((check) => check.id),
    ).toEqual(["native_host.manifest", "native_host.connection"]);
    expect(
      report.checks
        .filter((check) => check.status === "blocked")
        .map((check) => check.id),
    ).toEqual([
      "native_host.executable",
      "native_host.allowlist",
      "protocol.version",
      "native_host.version",
    ]);
    expect(report.repairActions).toEqual(["repair_registration"]);
  });

  it("repairs once with deduplicated actions and verifies the result", async () => {
    const broken = healthySnapshot();
    let current: BrowserIntegrationSnapshot = {
      ...broken,
      nativeHost: {
        ...broken.nativeHost,
        registeredBrowsers: [],
        manifest: null,
        reachable: false,
        protocolVersion: null,
        version: null,
      },
    };
    const repairs: BrowserIntegrationRepairAction[][] = [];
    let inspections = 0;
    const controller = new BrowserIntegrationDiagnosticsController(
      expectations,
      {
        async inspect() {
          inspections += 1;
          return current;
        },
        async repair(actions) {
          repairs.push([...actions]);
          current = healthySnapshot();
        },
      },
    );

    const before = await controller.runSelfCheck();
    expect(before.status).toBe("action_required");
    expect(controller.state.canRepair).toBe(true);

    const after = await controller.repair();
    expect(repairs).toEqual([["repair_registration"]]);
    expect(inspections).toBe(2);
    expect(after.status).toBe("healthy");
    expect(controller.state).toMatchObject({
      status: "ready",
      error: null,
      canRepair: false,
    });
  });

  it("preserves the actionable report when repair fails", async () => {
    const snapshot = healthySnapshot();
    const controller = new BrowserIntegrationDiagnosticsController(
      expectations,
      {
        async inspect() {
          return {
            ...snapshot,
            extension: { ...snapshot.extension, version: "9.0.0" },
          };
        },
        async repair() {
          throw new Error("installer is busy");
        },
      },
    );
    await controller.runSelfCheck();

    await expect(controller.repair()).rejects.toThrow("installer is busy");
    expect(controller.state).toMatchObject({
      status: "error",
      error: "installer is busy",
      canRepair: true,
    });
    expect(controller.state.report?.status).toBe("action_required");
  });
});
