/* global WebSocket, clearTimeout, fetch, setTimeout */

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import {
  access,
  cp,
  mkdir,
  readFile,
  readdir,
  stat,
  unlink,
  writeFile,
} from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import { isAbsolute, join, relative, resolve } from "node:path";

class CdpClient {
  static async connect(url, timeout = 5_000) {
    const client = new CdpClient(url);
    await new Promise((resolvePromise, rejectPromise) => {
      const timer = setTimeout(() => {
        try {
          client.socket.close();
        } catch {
          // The socket can still be in CONNECTING while a browser exits.
        }
        rejectPromise(new Error(`timed out connecting to ${url}`));
      }, timeout);
      client.ready.then(
        () => {
          clearTimeout(timer);
          resolvePromise();
        },
        (error) => {
          clearTimeout(timer);
          rejectPromise(error);
        },
      );
    });
    return client;
  }

  constructor(url) {
    this.nextId = 1;
    this.pending = new Map();
    this.socket = new WebSocket(url);
    this.ready = new Promise((resolvePromise, rejectPromise) => {
      this.socket.addEventListener("open", resolvePromise, { once: true });
      this.socket.addEventListener("error", rejectPromise, { once: true });
    });
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolvePromise, rejectPromise) => {
      this.pending.set(id, { resolve: resolvePromise, reject: rejectPromise });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.socket.close();
  }
}

const extensionId = "ackbonlbpniilcfmmnfdohnhbdbajhii";
const hostName = "io.github.musharaf_hossain_cs.download_manager";
const installedLocalAppData = process.env.LOCALAPPDATA;
if (!installedLocalAppData) throw new Error("LOCALAPPDATA is required");

const installRoot = join(installedLocalAppData, "Programs", "Download Manager");
const extensionRoot = join(installRoot, "browser-extension");
const desktopPath = join(installRoot, "download-manager-desktop.exe");
const backendPath = join(installRoot, "download-manager-backend.exe");
const cliPath = join(installRoot, "download-manager-cli.exe");
const acceptanceRoot = resolve("test-results", "pu12", "installed-browser");
const runRoot = join(acceptanceRoot, `run-${Date.now()}`);
const isolatedProfile = join(runRoot, "profile");
const isolatedLocalAppData = join(isolatedProfile, "AppData", "Local");
const isolatedAppData = join(isolatedProfile, "AppData", "Roaming");
const appDownloads = join(runRoot, "app-downloads");
const preferencesPath = join(
  isolatedLocalAppData,
  "DownloadManager",
  "preferences.json",
);
const manifestPath = join(runRoot, "fixture-manifest.json");
const identityPath = join(runRoot, "identity.txt");
const acceptanceEnvironment = {
  ...process.env,
  LOCALAPPDATA: isolatedLocalAppData,
  APPDATA: isolatedAppData,
  USERPROFILE: isolatedProfile,
};
const port = 49325;

const availableBrowsers = [
  {
    name: "chrome",
    executable:
      process.env.PU12_CHROME_PATH ??
      "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    selector: "#chrome-capture",
    expectedFilename: "pu12-chrome-capture.bin",
    profileTemplate: null,
    loadExtensionFromCommandLine: true,
  },
  {
    name: "edge",
    executable:
      "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    selector: "#edge-capture",
    expectedFilename: "pu12-edge-capture.bin",
    profileTemplate: process.env.PU12_EDGE_PROFILE ?? null,
    loadExtensionFromCommandLine: process.env.PU12_EDGE_PROFILE === undefined,
  },
];
const requestedBrowsers = new Set(
  (process.env.PU12_BROWSERS ?? "chrome,edge")
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean),
);
const browsers = availableBrowsers.filter((browser) =>
  requestedBrowsers.has(browser.name),
);
assert.equal(
  browsers.length,
  requestedBrowsers.size,
  "PU12_BROWSERS contains an unsupported browser",
);

for (const path of [extensionRoot, desktopPath, backendPath, cliPath]) {
  await access(path, fsConstants.R_OK);
}
for (const browser of browsers)
  await access(browser.executable, fsConstants.X_OK);

await mkdir(appDownloads, { recursive: true });
await mkdir(join(isolatedLocalAppData, "DownloadManager"), {
  recursive: true,
});
await writeFile(
  preferencesPath,
  `${JSON.stringify({ last_download_directory: appDownloads }, null, 2)}\n`,
  "utf8",
);

const fixture = spawn(
  process.execPath,
  [
    "scripts/private-use-acceptance-fixture.mjs",
    "--port",
    String(port),
    "--manifest",
    manifestPath,
    "--identity-file",
    identityPath,
  ],
  { cwd: resolve("."), stdio: ["ignore", "pipe", "pipe"] },
);
const children = new Set([fixture]);
const browserResults = [];

try {
  await waitForLine(fixture, '"ready":true', 10_000);
  const fixtureManifest = JSON.parse(await readFile(manifestPath, "utf8"));
  assert.equal(
    fixtureManifest.browser_page_url,
    `http://127.0.0.1:${port}/browser.html`,
  );

  await shutdownBackend();
  const backend = spawn(backendPath, [], {
    stdio: "ignore",
    windowsHide: true,
    env: acceptanceEnvironment,
  });
  children.add(backend);
  await waitForHealth();

  const baseline = await listJobs();
  assert.equal(
    baseline.length,
    0,
    "PU12 acceptance requires an empty installed job list",
  );

  for (const browser of browsers) {
    const result = await runBrowser(browser, fixtureManifest);
    browserResults.push(result);
  }

  const jobs = await listJobs();
  assert.equal(
    jobs.length,
    browsers.length,
    "each requested browser must create exactly one app job",
  );
  assert.deepEqual(
    jobs.map((job) => job.filename).sort(),
    browsers.map((browser) => browser.expectedFilename).sort(),
  );

  await shutdownBackend();

  const fallback = await runOfflineFallback(browsers[0], fixtureManifest);
  browserResults.push(fallback);

  const finalJobs = await listJobsOffline();
  assert.equal(
    finalJobs,
    null,
    "the backend must remain offline during fallback",
  );

  const report = {
    schema_version: 1,
    status: "passed",
    extension_id: extensionId,
    native_host: hostName,
    browsers: browserResults,
    app_jobs: jobs,
  };
  await writeFile(
    join(acceptanceRoot, "browser-acceptance.json"),
    `${JSON.stringify(report, null, 2)}\n`,
    "utf8",
  );
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
} finally {
  for (const child of children) {
    if (child && child.exitCode === null) child.kill();
  }
}

async function runBrowser(browser, fixtureManifest) {
  const profile = join(runRoot, `${browser.name}-profile`);
  const downloads = join(runRoot, `${browser.name}-downloads`);
  await prepareBrowserProfile(profile, downloads, browser.profileTemplate);
  const session = await launchBrowser(
    browser,
    profile,
    downloads,
    fixtureManifest.browser_page_url,
  );
  try {
    const initialHealth = await waitForExtensionBadge(session.port, 15_000);
    const page = await waitForTarget(
      session.port,
      (target) =>
        target.type === "page" &&
        target.url === fixtureManifest.browser_page_url,
      10_000,
    );
    await navigatePage(
      page.webSocketDebuggerUrl,
      fixtureManifest.browser_page_url,
    );
    const beforeJobs = await listJobs();
    const beforeIds = new Set(beforeJobs.map((job) => job.id));
    await clickSelector(page.webSocketDebuggerUrl, browser.selector);
    const jobs = await waitForJobs(beforeJobs.length + 1, 15_000);
    const captured = jobs.filter((job) => !beforeIds.has(job.id));
    assert.equal(
      captured.length,
      1,
      `${browser.name} created duplicate app jobs`,
    );
    await waitForJobCompletion(captured[0].id, 15_000);
    await waitForNoRegularFiles(downloads, 5_000);
    assert.deepEqual(await regularFiles(downloads), []);
    await delay(1_000);
    assert.equal(
      (await listJobs()).length,
      beforeJobs.length + 1,
      `${browser.name} produced a delayed duplicate app job`,
    );
    const { worker, badge } = initialHealth;
    assert.equal(
      badge.text,
      "✓",
      `${browser.name} did not report healthy native connectivity`,
    );
    assert.match(badge.title, /connected/u);

    return {
      browser: browser.name,
      version: await browserVersion(session.port),
      extension_url: worker.url,
      badge,
      captured_job_count: 1,
      duplicate_browser_files: 0,
    };
  } finally {
    await session.close();
  }
}

async function runOfflineFallback(browser, fixtureManifest) {
  const profile = join(runRoot, `${browser.name}-offline-profile`);
  const downloads = join(runRoot, `${browser.name}-offline-downloads`);
  await prepareBrowserProfile(profile, downloads, browser.profileTemplate);
  const session = await launchBrowser(
    browser,
    profile,
    downloads,
    fixtureManifest.browser_page_url,
  );
  try {
    const initialHealth = await waitForExtensionBadge(session.port, 15_000);
    const page = await waitForTarget(
      session.port,
      (target) =>
        target.type === "page" &&
        target.url === fixtureManifest.browser_page_url,
      10_000,
    );
    await navigatePage(
      page.webSocketDebuggerUrl,
      fixtureManifest.browser_page_url,
    );
    await clickSelector(page.webSocketDebuggerUrl, "#offline-fallback");
    const fallbackPath = join(downloads, "pu12-browser-fallback.txt");
    await waitForFile(fallbackPath, 15_000);
    const actualHash = sha256(await readFile(fallbackPath));
    assert.equal(actualHash, fixtureManifest.fixtures.fallback.sha256);
    const { badge } = initialHealth;
    assert.equal(
      badge.text,
      "✓",
      "native-host health must remain local while the backend is offline",
    );
    return {
      browser: browser.name,
      mode: "backend_offline",
      native_health_badge: badge.text,
      browser_fallback_file: fallbackPath,
      sha256: actualHash,
    };
  } finally {
    await session.close();
  }
}

async function launchBrowser(browser, profile, downloads, url) {
  const activePortPath = join(profile, "DevToolsActivePort");
  await unlink(activePortPath).catch((error) => {
    if (error?.code !== "ENOENT") throw error;
  });
  const arguments_ = [
    `--user-data-dir=${profile}`,
    "--remote-debugging-port=0",
    "--no-first-run",
    "--disable-sync",
    "--disable-default-apps",
  ];
  if (browser.name === "edge") {
    arguments_.push("--disable-features=msEdgeSignin");
  }
  if (browser.loadExtensionFromCommandLine) {
    arguments_.push(
      `--disable-extensions-except=${extensionRoot}`,
      `--load-extension=${extensionRoot}`,
    );
  }
  arguments_.push("--new-window", url);
  const child = spawn(browser.executable, arguments_, {
    stdio: "ignore",
    env: acceptanceEnvironment,
  });
  children.add(child);
  await waitForFile(activePortPath, 15_000);
  const [portLine, browserPath] = (await readFile(activePortPath, "utf8"))
    .trim()
    .split(/\r?\n/u);
  const debugPort = Number(portLine);
  assert.ok(Number.isInteger(debugPort) && debugPort > 0);
  const browserClient = await CdpClient.connect(
    `ws://127.0.0.1:${debugPort}${browserPath}`,
  );
  await browserClient.send("Browser.setDownloadBehavior", {
    behavior: "allow",
    downloadPath: downloads,
    eventsEnabled: true,
  });
  browserClient.close();
  return {
    port: debugPort,
    close: async () => {
      try {
        const client = await Promise.race([
          CdpClient.connect(`ws://127.0.0.1:${debugPort}${browserPath}`),
          delay(500).then(() => null),
        ]);
        if (client === null) {
          if (child.exitCode === null) child.kill();
        } else {
          await Promise.race([client.send("Browser.close"), delay(500)]);
          client.close();
        }
      } catch {
        if (child.exitCode === null) child.kill();
      }
      await waitForExit(child, 5_000);
      children.delete(child);
    },
  };
}

async function prepareBrowserProfile(
  profile,
  downloads,
  profileTemplate = null,
) {
  await mkdir(downloads, { recursive: true });
  if (profileTemplate !== null) {
    const allowedRoot = resolve("test-results", "pu12");
    const resolvedTemplate = resolve(profileTemplate);
    const templateRelative = relative(allowedRoot, resolvedTemplate);
    assert.ok(
      templateRelative.length > 0 &&
        !templateRelative.startsWith("..") &&
        !isAbsolute(templateRelative),
      "the Edge profile template must be inside test-results/pu12",
    );
    await mkdir(join(profile, "Default"), { recursive: true });
    for (const name of ["Preferences", "Secure Preferences"]) {
      await cp(
        join(resolvedTemplate, "Default", name),
        join(profile, "Default", name),
      );
    }
    const preferencesPath = join(profile, "Default", "Preferences");
    const preferences = JSON.parse(await readFile(preferencesPath, "utf8"));
    preferences.download = {
      ...preferences.download,
      default_directory: downloads,
      prompt_for_download: false,
    };
    preferences.savefile = {
      ...preferences.savefile,
      default_directory: downloads,
    };
    await writeFile(preferencesPath, JSON.stringify(preferences), "utf8");
    await cp(
      join(resolvedTemplate, "Local State"),
      join(profile, "Local State"),
    );
    for (const name of [
      "Extension Rules",
      "Extension Scripts",
      "Extension State",
      "Local Extension Settings",
      "Sync Extension Settings",
    ]) {
      const source = join(resolvedTemplate, "Default", name);
      try {
        await cp(source, join(profile, "Default", name), { recursive: true });
      } catch (error) {
        if (error?.code !== "ENOENT") throw error;
      }
    }
    return;
  }
  await mkdir(join(profile, "Default"), { recursive: true });
  await writeFile(
    join(profile, "Default", "Preferences"),
    JSON.stringify({
      download: {
        default_directory: downloads,
        prompt_for_download: false,
      },
    }),
    "utf8",
  );
}

async function readBadge(webSocketUrl) {
  const client = await CdpClient.connect(webSocketUrl);
  try {
    await client.send("Runtime.enable");
    const result = await client.send("Runtime.evaluate", {
      expression:
        "new Promise((resolve) => chrome.action.getBadgeText({}, (text) => chrome.action.getTitle({}, (title) => resolve({text,title}))))",
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails || result.result?.value === undefined) {
      throw new Error(
        `could not read extension badge: ${JSON.stringify(result)}`,
      );
    }
    return result.result.value;
  } finally {
    client.close();
  }
}

async function wakeExtension(webSocketUrl) {
  const client = await CdpClient.connect(webSocketUrl);
  try {
    await client.send("Runtime.enable");
    await client.send("Runtime.evaluate", {
      expression:
        "new Promise((resolve) => chrome.runtime.sendMessage({type: 'pu12-wake'}, () => { void chrome.runtime.lastError; resolve(true); }))",
      awaitPromise: true,
      returnByValue: true,
    });
  } finally {
    client.close();
  }
}

async function waitForExtensionBadge(portNumber, timeout) {
  const deadline = Date.now() + timeout;
  let lastTargets = [];
  let extensionPageRequested = false;
  do {
    let targets;
    try {
      targets = await fetch(`http://127.0.0.1:${portNumber}/json/list`).then(
        (response) => response.json(),
      );
      lastTargets = targets;
    } catch {
      await delay(100);
      continue;
    }
    let worker = targets.find(
      (target) =>
        target.type === "service_worker" &&
        target.url ===
          `chrome-extension://${extensionId}/src/service-worker.js`,
    );
    if (!worker) {
      worker = targets.find(
        (target) =>
          target.type === "page" &&
          target.url.startsWith(`chrome-extension://${extensionId}/`),
      );
    }
    if (!worker && !extensionPageRequested) {
      extensionPageRequested = true;
      try {
        await fetch(
          `http://127.0.0.1:${portNumber}/json/new?${encodeURIComponent(`chrome-extension://${extensionId}/manifest.json`)}`,
          { method: "PUT" },
        );
      } catch {
        // Target discovery below reports a bounded failure if the extension is absent.
      }
    }
    if (worker) {
      try {
        if (worker.type === "page") {
          await wakeExtension(worker.webSocketDebuggerUrl);
          await delay(100);
          continue;
        }
        const badge = await readBadge(worker.webSocketDebuggerUrl);
        if (["✓", "!", "↕", "OFF"].includes(badge.text)) {
          return { worker, badge };
        }
      } catch {
        // A Manifest V3 worker can be replaced between target discovery and attach.
      }
    }
    await delay(100);
  } while (Date.now() < deadline);
  throw new Error(
    `timed out waiting for the extension health badge: ${JSON.stringify(lastTargets.map(({ type, url, title }) => ({ type, url, title })))}`,
  );
}

async function navigatePage(webSocketUrl, url) {
  const client = await CdpClient.connect(webSocketUrl);
  try {
    await client.send("Page.enable");
    await client.send("Runtime.enable");
    const deadline = Date.now() + 10_000;
    do {
      const result = await client.send("Page.navigate", { url });
      if (result.errorText && result.errorText !== "net::ERR_ABORTED") {
        throw new Error(`browser navigation failed: ${result.errorText}`);
      }
      await delay(100);
      const location = await client.send("Runtime.evaluate", {
        expression: "document.URL",
        returnByValue: true,
      });
      if (location.result?.value === url) return;
    } while (Date.now() < deadline);
    throw new Error(`browser did not finish navigation to ${url}`);
  } finally {
    client.close();
  }
}

async function clickSelector(webSocketUrl, selector) {
  const client = await CdpClient.connect(webSocketUrl);
  try {
    await client.send("Page.bringToFront");
    await client.send("Runtime.enable");
    const deadline = Date.now() + 10_000;
    let point;
    let lastResult;
    do {
      lastResult = await client.send("Runtime.evaluate", {
        expression: `(() => { const element = document.querySelector(${JSON.stringify(selector)}); if (!element) return null; const rect = element.getBoundingClientRect(); return {x: rect.left + rect.width / 2, y: rect.top + rect.height / 2}; })()`,
        returnByValue: true,
      });
      point = lastResult.result?.value;
      if (point) break;
      await delay(100);
    } while (Date.now() < deadline);
    if (!point) {
      const pageState = await client.send("Runtime.evaluate", {
        expression:
          "({url: document.URL, title: document.title, body: document.body?.innerHTML ?? null})",
        returnByValue: true,
      });
      throw new Error(
        `could not locate acceptance link: ${JSON.stringify({ lastResult, pageState })}`,
      );
    }
    const { x, y } = point;
    const hit = await client.send("Runtime.evaluate", {
      expression: `document.elementFromPoint(${x}, ${y})?.matches(${JSON.stringify(selector)}) ?? false`,
      returnByValue: true,
    });
    assert.equal(
      hit.result?.value,
      true,
      "acceptance click point missed its link",
    );
    const activation = await client.send("Runtime.evaluate", {
      expression: `(() => { document.querySelector(${JSON.stringify(selector)}).click(); return true; })()`,
      returnByValue: true,
    });
    assert.equal(
      activation.result?.value,
      true,
      "browser link activation failed",
    );
  } finally {
    client.close();
  }
}

async function waitForTarget(portNumber, predicate, timeout) {
  const deadline = Date.now() + timeout;
  do {
    let targets;
    try {
      targets = await fetch(`http://127.0.0.1:${portNumber}/json/list`).then(
        (response) => response.json(),
      );
    } catch {
      await delay(100);
      continue;
    }
    const target = targets.find(predicate);
    if (target) return target;
    await delay(100);
  } while (Date.now() < deadline);
  throw new Error("timed out waiting for browser target");
}

async function browserVersion(portNumber) {
  const deadline = Date.now() + 5_000;
  do {
    try {
      const version = await fetch(
        `http://127.0.0.1:${portNumber}/json/version`,
      ).then((response) => response.json());
      return version.Browser;
    } catch {
      await delay(100);
    }
  } while (Date.now() < deadline);
  throw new Error("timed out reading browser version");
}

async function waitForHealth() {
  const deadline = Date.now() + 15_000;
  do {
    const result = await runCli(["health", "--json"], true);
    if (result?.ok) return;
    await delay(100);
  } while (Date.now() < deadline);
  throw new Error("installed backend did not become healthy");
}

async function shutdownBackend() {
  await runCli(["shutdown", "--json"], true);
  await delay(250);
}

async function listJobs() {
  const result = await runCli(["list", "--json"]);
  assert.equal(result.ok, true);
  return result.data;
}

async function listJobsOffline() {
  return runCli(["list", "--json"], true);
}

async function waitForJobs(count, timeout) {
  const deadline = Date.now() + timeout;
  do {
    const jobs = await listJobs();
    if (jobs.length === count) return jobs;
    if (jobs.length > count)
      throw new Error("browser capture created duplicate jobs");
    await delay(100);
  } while (Date.now() < deadline);
  throw new Error(`timed out waiting for ${count} jobs`);
}

async function waitForJobCompletion(id, timeout) {
  assert.ok(id, "captured job ID is required");
  const deadline = Date.now() + timeout;
  do {
    const result = await runCli(["status", id, "--json"]);
    if (result.data.state === "completed") return;
    if (result.data.state === "failed")
      throw new Error(`captured job failed: ${result.data.failure_reason}`);
    await delay(100);
  } while (Date.now() < deadline);
  throw new Error(`timed out waiting for captured job ${id}`);
}

async function runCli(arguments_, allowFailure = false) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(cliPath, arguments_, {
      stdio: ["ignore", "pipe", "pipe"],
      env: acceptanceEnvironment,
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk) => (stderr += chunk));
    child.once("error", rejectPromise);
    child.once("exit", (code) => {
      if (code !== 0) {
        if (allowFailure) return resolvePromise(null);
        return rejectPromise(new Error(`CLI exited ${code}: ${stderr}`));
      }
      try {
        resolvePromise(JSON.parse(stdout));
      } catch (error) {
        rejectPromise(error);
      }
    });
  });
}

async function regularFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  return entries
    .filter((entry) => entry.isFile())
    .map((entry) => entry.name)
    .sort();
}

async function waitForFile(path, timeout) {
  const deadline = Date.now() + timeout;
  do {
    try {
      const details = await stat(path);
      if (details.isFile() && details.size > 0) return;
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
    await delay(100);
  } while (Date.now() < deadline);
  throw new Error(`timed out waiting for file: ${path}`);
}

async function waitForNoRegularFiles(path, timeout) {
  const deadline = Date.now() + timeout;
  do {
    if ((await regularFiles(path)).length === 0) return;
    await delay(100);
  } while (Date.now() < deadline);
  throw new Error(`timed out waiting for browser download cleanup: ${path}`);
}

async function waitForLine(child, text, timeout) {
  return new Promise((resolvePromise, rejectPromise) => {
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(
      () => rejectPromise(new Error(`process not ready: ${stderr}`)),
      timeout,
    );
    child.stdout.setEncoding("utf8").on("data", (chunk) => {
      stdout += chunk;
      if (stdout.includes(text)) {
        clearTimeout(timer);
        resolvePromise();
      }
    });
    child.stderr.setEncoding("utf8").on("data", (chunk) => (stderr += chunk));
    child.once("exit", (code) => {
      clearTimeout(timer);
      rejectPromise(new Error(`process exited ${code}: ${stderr}`));
    });
  });
}

async function waitForExit(child, timeout) {
  if (child.exitCode !== null) return;
  await Promise.race([
    new Promise((resolvePromise) => child.once("exit", resolvePromise)),
    delay(timeout).then(() => {
      if (child.exitCode === null) child.kill();
    }),
  ]);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function delay(milliseconds) {
  return new Promise((resolvePromise) =>
    setTimeout(resolvePromise, milliseconds),
  );
}
