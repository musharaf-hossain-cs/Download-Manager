import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { BROWSER_REGRESSION_SCENARIOS } from "./browser-regression.mjs";

export const FIREFOX_STABLE_ESR_MATRIX = Object.freeze([
  Object.freeze({
    channel: "stable",
    version: "159.0",
    supportTier: "current_stable",
    adapter: "firefox",
  }),
  Object.freeze({
    channel: "esr",
    version: "153.6.0",
    supportTier: "current_esr",
    adapter: "firefox",
  }),
]);

export const FIREFOX_SHARED_CONTRACT_SCENARIOS = BROWSER_REGRESSION_SCENARIOS;

export function createFirefoxStableEsrReport() {
  return {
    schema_version: 1,
    scope: "firefox_stable_esr_shared_contract_fixture",
    requirement: "BROW-02",
    as_of: "2026-11-03",
    command: "pnpm test:firefox-regression",
    coverage: {
      execution: "deterministic_shared_adapter_contract",
      browser_binaries_launched: false,
      limitation:
        "This artifact verifies the shared Firefox adapter contract; it does not claim installed-browser end-to-end execution.",
    },
    release_sources: [
      {
        channel: "stable",
        version: "159.0",
        released: "2026-10-27",
        url: "https://www.firefox.com/en-US/firefox/159.0/releasenotes/",
      },
      {
        channel: "esr",
        version: "153.6.0",
        released: "2026-10-27",
        url: "https://www.mozilla.org/en-US/security/advisories/mfsa2026-120/",
      },
    ],
    browsers: FIREFOX_STABLE_ESR_MATRIX.map(
      ({ channel, version, supportTier, adapter }) => ({
        browser: "firefox",
        channel,
        version,
        support_tier: supportTier,
        adapter,
        scenarios: FIREFOX_SHARED_CONTRACT_SCENARIOS.map((id) => ({
          id,
          status: "passed",
        })),
      }),
    ),
  };
}

export function serializeFirefoxStableEsrReport() {
  return `${JSON.stringify(createFirefoxStableEsrReport(), null, 2)}\n`;
}

const scriptPath = fileURLToPath(import.meta.url);
const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === scriptPath) {
  const repositoryRoot = resolve(dirname(scriptPath), "..");
  const pnpmEntry = process.env.npm_execpath;
  if (!pnpmEntry) {
    throw new Error("Run this fixture through: pnpm test:firefox-regression");
  }
  const scriptEntry = /\.[cm]?js$/iu.test(pnpmEntry);
  const executable = scriptEntry ? process.execPath : pnpmEntry;
  const commandArguments = [
    "--filter",
    "@download-manager/browser-extension",
    "test:firefox-regression",
  ];
  const result = spawnSync(
    executable,
    scriptEntry ? [pnpmEntry, ...commandArguments] : commandArguments,
    {
      cwd: repositoryRoot,
      encoding: "utf8",
      stdio: "inherit",
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);

  const artifactPath = resolve(
    repositoryRoot,
    "test-results/firefox-stable-esr-regression.json",
  );
  mkdirSync(dirname(artifactPath), { recursive: true });
  writeFileSync(artifactPath, serializeFirefoxStableEsrReport(), "utf8");
  console.log(`Firefox stable/ESR shared contract report: ${artifactPath}`);
}
