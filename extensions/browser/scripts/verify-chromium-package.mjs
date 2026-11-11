import { createHash } from "node:crypto";
import { Buffer } from "node:buffer";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { stdout } from "node:process";
import { URL } from "node:url";

const packageRoot = new URL("../dist/chromium/", import.meta.url);
const repositoryRoot = new URL("../../../", import.meta.url);
const manifest = JSON.parse(
  readFileSync(new URL("manifest.json", packageRoot), "utf8"),
);
const extensionId = extensionIdFromKey(manifest.key);
const expectedId = "ackbonlbpniilcfmmnfdohnhbdbajhii";

if (manifest.manifest_version !== 3) {
  throw new Error("Chromium package must use Manifest V3");
}
if (manifest.background?.service_worker !== "src/service-worker.js") {
  throw new Error(
    "Chromium package does not declare the emitted service worker",
  );
}
if (!existsSync(new URL(manifest.background.service_worker, packageRoot))) {
  throw new Error("Chromium package service worker is missing");
}
if (extensionId !== expectedId) {
  throw new Error(
    `Chromium package ID ${extensionId} does not match ${expectedId}`,
  );
}
if (
  manifest.host_permissions !== undefined ||
  manifest.content_scripts !== undefined
) {
  throw new Error("Chromium package must not request broad site access");
}
if (
  JSON.stringify(manifest.permissions) !==
  JSON.stringify(["nativeMessaging", "downloads", "contextMenus", "storage"])
) {
  throw new Error("Chromium package permissions exceed the PU11 runtime scope");
}
const sourceFiles = readdirSync(new URL("src/", packageRoot), {
  recursive: true,
  withFileTypes: true,
})
  .filter((entry) => entry.isFile())
  .map((entry) => join(entry.parentPath, entry.name));
if (sourceFiles.some((path) => /media|overlay|firefox/iu.test(path))) {
  throw new Error("Chromium package contains out-of-scope browser modules");
}
for (const relativePath of [
  "crates/download-manager-native-host/src/main.rs",
  "packaging/windows/native-host-registration.ps1",
  "scripts/native-host-dev.ps1",
]) {
  const contents = readFileSync(new URL(relativePath, repositoryRoot), "utf8");
  if (!contents.includes(expectedId)) {
    throw new Error(`${relativePath} does not bind the Chromium package ID`);
  }
}

stdout.write(`Chromium load-unpacked package verified (${extensionId}).\n`);

function extensionIdFromKey(key) {
  if (typeof key !== "string" || key.length === 0) {
    throw new Error("Chromium manifest key is missing");
  }
  const digest = createHash("sha256")
    .update(Buffer.from(key, "base64"))
    .digest();
  return [...digest.subarray(0, 16)]
    .flatMap((byte) => [byte >>> 4, byte & 0x0f])
    .map((nibble) => String.fromCharCode("a".charCodeAt(0) + nibble))
    .join("");
}
