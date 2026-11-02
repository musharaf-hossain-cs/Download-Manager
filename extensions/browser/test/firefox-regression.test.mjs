import { describe, expect, it } from "vitest";

import { BROWSER_REGRESSION_SCENARIOS } from "../../../scripts/browser-regression.mjs";
import {
  FIREFOX_SHARED_CONTRACT_SCENARIOS,
  FIREFOX_STABLE_ESR_MATRIX,
  createFirefoxStableEsrReport,
  serializeFirefoxStableEsrReport,
} from "../../../scripts/firefox-regression.mjs";

describe("Firefox stable and ESR shared contract report", () => {
  it("binds the current stable and ESR releases to the Firefox adapter", () => {
    expect(FIREFOX_STABLE_ESR_MATRIX).toEqual([
      {
        channel: "stable",
        version: "159.0",
        supportTier: "current_stable",
        adapter: "firefox",
      },
      {
        channel: "esr",
        version: "153.6.0",
        supportTier: "current_esr",
        adapter: "firefox",
      },
    ]);
  });

  it("reports exactly the scenarios exercised by the shared adapter matrix", () => {
    expect(FIREFOX_SHARED_CONTRACT_SCENARIOS).toBe(
      BROWSER_REGRESSION_SCENARIOS,
    );
    expect(FIREFOX_SHARED_CONTRACT_SCENARIOS).toEqual([
      "handoff.accepted_once",
      "handoff.fallback_preserves_url",
      "capture.rules_and_modifiers",
      "menus.single_all_selected",
    ]);
  });

  it("serializes a stable, dated, explicitly fixture-scoped artifact", () => {
    expect(serializeFirefoxStableEsrReport()).toBe(
      serializeFirefoxStableEsrReport(),
    );
    expect(createFirefoxStableEsrReport()).toMatchObject({
      schema_version: 1,
      scope: "firefox_stable_esr_shared_contract_fixture",
      requirement: "BROW-02",
      as_of: "2026-11-03",
      command: "pnpm test:firefox-regression",
      coverage: {
        execution: "deterministic_shared_adapter_contract",
        browser_binaries_launched: false,
      },
      release_sources: [
        {
          channel: "stable",
          version: "159.0",
          released: "2026-10-27",
        },
        {
          channel: "esr",
          version: "153.6.0",
          released: "2026-10-27",
        },
      ],
      browsers: [
        {
          browser: "firefox",
          channel: "stable",
          version: "159.0",
          support_tier: "current_stable",
          adapter: "firefox",
        },
        {
          browser: "firefox",
          channel: "esr",
          version: "153.6.0",
          support_tier: "current_esr",
          adapter: "firefox",
        },
      ],
    });
    for (const browser of createFirefoxStableEsrReport().browsers) {
      expect(browser.scenarios).toEqual(
        FIREFOX_SHARED_CONTRACT_SCENARIOS.map((id) => ({
          id,
          status: "passed",
        })),
      );
    }
  });
});
