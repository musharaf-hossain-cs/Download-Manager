import { describe, expect, it } from "vitest";

import {
  BROWSER_ROUTE_INHERITANCE_LIMITATION,
  BrowserNativeRouteDiagnostics,
  evaluateBrowserNativeRoute,
  type BrowserNativeRouteReport,
  type RouteObservation,
} from "../src/index.js";

const browserRoute: RouteObservation = {
  proxyMode: "system",
  egressFingerprint: "browser-route-01",
};

describe("browser/native route diagnostics", () => {
  it("detects differing proxy/VPN route evidence and shows the limitation", async () => {
    const shown: BrowserNativeRouteReport[] = [];
    const diagnostics = new BrowserNativeRouteDiagnostics(
      {
        async observeBrowserRoute() {
          return browserRoute;
        },
        async observeNativeRoute() {
          return {
            proxyMode: "direct",
            egressFingerprint: "native-route--02",
          };
        },
      },
      {
        showRouteDiagnostic(report) {
          shown.push(report);
        },
      },
    );

    await expect(diagnostics.check()).resolves.toMatchObject({
      compatibility: "mismatch",
      limitation: BROWSER_ROUTE_INHERITANCE_LIMITATION,
      explanation: expect.stringContaining("different"),
      recommendedAction: expect.stringContaining("browser"),
    });
    expect(shown).toHaveLength(1);
    expect(shown[0]?.compatibility).toBe("mismatch");
  });

  it("attempts both observations and reports indeterminate when one fails", async () => {
    const calls: string[] = [];
    const shown: BrowserNativeRouteReport[] = [];
    const diagnostics = new BrowserNativeRouteDiagnostics(
      {
        async observeBrowserRoute() {
          calls.push("browser");
          throw new Error("browser route API unavailable");
        },
        async observeNativeRoute() {
          calls.push("native");
          return browserRoute;
        },
      },
      { showRouteDiagnostic: (report) => shown.push(report) },
    );

    await expect(diagnostics.check()).resolves.toMatchObject({
      compatibility: "indeterminate",
      browser: null,
      native: browserRoute,
      limitation: BROWSER_ROUTE_INHERITANCE_LIMITATION,
    });
    expect(calls.sort()).toEqual(["browser", "native"]);
    expect(shown[0]?.explanation).not.toContain(
      "browser route API unavailable",
    );
  });

  it("treats equal opaque egress samples as provisional, not inheritance", () => {
    const report = evaluateBrowserNativeRoute(browserRoute, {
      proxyMode: "system",
      egressFingerprint: "browser-route-01",
    });

    expect(report).toMatchObject({
      compatibility: "matched",
      limitation: BROWSER_ROUTE_INHERITANCE_LIMITATION,
      explanation: expect.stringContaining("later request"),
    });
  });

  it("does not expose raw addresses or malformed route evidence", () => {
    const report = evaluateBrowserNativeRoute(
      {
        proxyMode: "manual",
        egressFingerprint: "203.0.113.9:8080",
      },
      browserRoute,
    );

    expect(report.compatibility).toBe("indeterminate");
    expect(report.browser).toBeNull();
    expect(JSON.stringify(report)).not.toContain("203.0.113.9");
  });
});
