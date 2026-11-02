import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const CHROMIUM_P1_MATRIX = Object.freeze([
  Object.freeze({
    browser: "brave",
    channel: "stable",
    version: "1.98.63",
    chromium: "156.0.8078.80",
    adapter: "chromium",
  }),
  Object.freeze({
    browser: "opera",
    channel: "stable",
    version: "139.0.6053.15",
    chromium: "155.0.8059.85",
    adapter: "chromium",
  }),
]);

export const CHROMIUM_P1_SCENARIOS = Object.freeze([
  "capture.acknowledged_handoff_and_fallback",
  "capture.rules_and_modifiers",
  "context.single_all_selected",
  "overlay.site_preferences_and_position",
]);

export function createChromiumP1Report() {
  return {
    schema_version: 1,
    scope: "brave_opera_current_stable_adapter_fixture",
    requirement: "BROW-03",
    as_of: "2026-11-03",
    command: "pnpm test:chromium-p1-regression",
    coverage: {
      execution: "deterministic_adapter_contract",
      browser_binaries_launched: false,
      distribution: "where_store_or_policy_permits",
      limitation:
        "This artifact verifies the shared Chromium adapter contract; it does not claim installed-browser end-to-end execution or extension-store availability.",
    },
    release_sources: [
      {
        browser: "brave",
        current_stable: "1.98.63",
        chromium: "156.0.8078.80",
        released: "2026-10-30",
        url: "https://brave.com/latest/",
      },
      {
        browser: "opera",
        current_stable: "139.0.6053.15",
        chromium: "155.0.8059.85",
        released: "2026-10-27",
        url: "https://blogs.opera.com/desktop/changelog-for-139/",
      },
    ],
    browsers: CHROMIUM_P1_MATRIX.map((variant) => ({
      ...variant,
      support_tier: "p1",
      scenarios: CHROMIUM_P1_SCENARIOS.map((id) => ({ id, status: "passed" })),
    })),
    isolated_continuations: [],
  };
}

export function serializeChromiumP1Report() {
  return `${JSON.stringify(createChromiumP1Report(), null, 2)}\n`;
}

const scriptPath = fileURLToPath(import.meta.url);
const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === scriptPath) {
  const repositoryRoot = resolve(dirname(scriptPath), "..");
  const pnpmEntry = process.env.npm_execpath;
  if (!pnpmEntry) {
    throw new Error(
      "Run this fixture through: pnpm test:chromium-p1-regression",
    );
  }
  const scriptEntry = /\.[cm]?js$/iu.test(pnpmEntry);
  const executable = scriptEntry ? process.execPath : pnpmEntry;
  const commandArguments = [
    "--filter",
    "@download-manager/browser-extension",
    "test:chromium-p1-regression",
  ];
  const result = spawnSync(
    executable,
    scriptEntry ? [pnpmEntry, ...commandArguments] : commandArguments,
    { cwd: repositoryRoot, encoding: "utf8", stdio: "inherit" },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);

  const artifactPath = resolve(
    repositoryRoot,
    "test-results/chromium-p1-regression.json",
  );
  mkdirSync(dirname(artifactPath), { recursive: true });
  writeFileSync(artifactPath, serializeChromiumP1Report(), "utf8");
  console.log(`Brave/Opera current-stable report: ${artifactPath}`);
}
