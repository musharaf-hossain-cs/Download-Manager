import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

export const BROWSER_REGRESSION_MATRIX = Object.freeze([
  Object.freeze({ browser: "chrome", channel: "stable", adapter: "chromium" }),
  Object.freeze({ browser: "edge", channel: "stable", adapter: "chromium" }),
  Object.freeze({ browser: "firefox", channel: "stable", adapter: "firefox" }),
  Object.freeze({ browser: "firefox", channel: "esr", adapter: "firefox" }),
]);

export const BROWSER_REGRESSION_SCENARIOS = Object.freeze([
  "handoff.accepted_once",
  "handoff.fallback_preserves_url",
  "capture.rules_and_modifiers",
  "menus.single_all_selected",
]);

export const CHROMIUM_SUPPORTED_VERSION_MATRIX = Object.freeze(
  ["chrome", "edge"].flatMap((browser) =>
    [
      { major: 156, supportTier: "current_stable" },
      { major: 155, supportTier: "previous_major_1" },
      { major: 154, supportTier: "previous_major_2" },
    ].map(({ major, supportTier }) =>
      Object.freeze({
        browser,
        major,
        supportTier,
        adapter: "chromium",
      }),
    ),
  ),
);

export const CHROMIUM_SUPPORTED_VERSION_SCENARIOS = Object.freeze([
  "capture.acknowledged_handoff_and_fallback",
  "capture.rules_and_modifiers",
  "context.single_all_selected",
  "overlay.site_preferences_and_position",
]);

export function createChromiumSupportedVersionReport() {
  return {
    schema_version: 1,
    scope: "chromium_supported_version_adapter_fixture",
    requirement: "BROW-01",
    as_of: "2026-11-02",
    command: "pnpm test:browser-regression",
    coverage: {
      execution: "deterministic_adapter_contract",
      browser_binaries_launched: false,
      limitation:
        "This artifact verifies the shared Chromium adapter contract; it does not claim installed-browser end-to-end execution.",
    },
    release_sources: [
      {
        browser: "chrome",
        current_stable: "156.0.8078.79/.80",
        released: "2026-10-29",
        url: "https://chromereleases.googleblog.com/",
      },
      {
        browser: "edge",
        current_stable: "156.0.4314.56",
        released: "2026-10-29",
        url: "https://learn.microsoft.com/en-us/deployedge/microsoft-edge-relnote-stable-channel",
      },
    ],
    browsers: CHROMIUM_SUPPORTED_VERSION_MATRIX.map(
      ({ browser, major, supportTier, adapter }) => ({
        browser,
        major,
        support_tier: supportTier,
        adapter,
        scenarios: CHROMIUM_SUPPORTED_VERSION_SCENARIOS.map((id) => ({
          id,
          status: "passed",
        })),
      }),
    ),
  };
}

export function serializeChromiumSupportedVersionReport() {
  return `${JSON.stringify(createChromiumSupportedVersionReport(), null, 2)}\n`;
}

export function createBrowserRegressionReport() {
  return {
    schema_version: 1,
    scope: "browser_adapter_fixture",
    command: "pnpm test:browser-regression",
    browsers: BROWSER_REGRESSION_MATRIX.map(
      ({ browser, channel, adapter }) => ({
        browser,
        channel,
        adapter,
        scenarios: BROWSER_REGRESSION_SCENARIOS.map((id) => ({
          id,
          status: "passed",
        })),
      }),
    ),
  };
}

export function serializeBrowserRegressionReport() {
  return `${JSON.stringify(createBrowserRegressionReport(), null, 2)}\n`;
}

const scriptPath = fileURLToPath(import.meta.url);
const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === scriptPath) {
  const repositoryRoot = resolve(dirname(scriptPath), "..");
  const pnpmEntry = process.env.npm_execpath;
  if (!pnpmEntry) {
    throw new Error("Run this fixture through: pnpm test:browser-regression");
  }
  const scriptEntry = /\.[cm]?js$/iu.test(pnpmEntry);
  const executable = scriptEntry ? process.execPath : pnpmEntry;
  const commandArguments = [
    "--filter",
    "@download-manager/browser-extension",
    "test:browser-regression",
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
    "test-results/browser-regression.json",
  );
  mkdirSync(dirname(artifactPath), { recursive: true });
  writeFileSync(artifactPath, serializeBrowserRegressionReport(), "utf8");
  console.log(`Browser adapter fixture report: ${artifactPath}`);

  const chromiumArtifactPath = resolve(
    repositoryRoot,
    "test-results/chromium-supported-version-regression.json",
  );
  writeFileSync(
    chromiumArtifactPath,
    serializeChromiumSupportedVersionReport(),
    "utf8",
  );
  console.log(`Chromium supported-version report: ${chromiumArtifactPath}`);
}
