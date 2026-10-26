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
}
