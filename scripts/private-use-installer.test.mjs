import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const configuration = JSON.parse(
  await readFile("apps/desktop/src-tauri/tauri.conf.json", "utf8"),
);
const hooks = await readFile("apps/desktop/src-tauri/nsis-hooks.nsh", "utf8");
const buildScript = await readFile(
  "scripts/build-private-use-installer.ps1",
  "utf8",
);
const registrationScript = await readFile(
  "packaging/windows/native-host-registration.ps1",
  "utf8",
);
const browserInstructions = await readFile(
  "packaging/windows/PRIVATE_BROWSER_SETUP.txt",
  "utf8",
);
const storage = await readFile(
  "crates/download-manager-engine/src/storage.rs",
  "utf8",
);

assert.equal(configuration.bundle.active, true);
assert.equal(configuration.bundle.targets, "nsis");
assert.deepEqual(configuration.bundle.externalBin, [
  "binaries/download-manager-backend",
  "binaries/download-manager-cli",
  "binaries/download-manager-native-host",
]);
assert.deepEqual(configuration.bundle.resources, {
  "../../../packaging/windows/native-host-registration.ps1":
    "native-host-registration.ps1",
  "../../../packaging/windows/PRIVATE_BROWSER_SETUP.txt": "BROWSER_SETUP.txt",
  "../../../extensions/browser/dist/chromium/": "browser-extension/",
});
assert.equal(configuration.bundle.windows.nsis.installMode, "currentUser");
assert.equal(
  configuration.bundle.windows.nsis.installerHooks,
  "nsis-hooks.nsh",
);
assert.equal(configuration.bundle.windows.webviewInstallMode.type, "skip");
assert.doesNotMatch(JSON.stringify(configuration.bundle), /ffmpeg/i);

assert.match(hooks, /^CRCCheck force$/m);
assert.match(
  hooks,
  /StrCpy \$INSTDIR "\$LOCALAPPDATA\\Programs\\\$\{PRODUCTNAME\}"/,
);
assert.match(hooks, /SetOutPath \$INSTDIR/);
assert.match(hooks, /NSIS_HOOK_POSTINSTALL/);
assert.match(hooks, /-Action Register -Browser Chromium/);
assert.match(hooks, /-Action Unregister -Browser Chromium/);
assert.ok(
  hooks.indexOf("-Action Unregister") < hooks.indexOf("IfSilent dm_keep_state"),
);
assert.match(hooks, /IfSilent dm_keep_state/);
assert.match(hooks, /Completed downloads are never removed/);
assert.doesNotMatch(hooks, /RMDir \/r "\$LOCALAPPDATA\\DownloadManager"/);

assert.match(buildScript, /rustc -vV/);
assert.match(buildScript, /--locked --release -p download-manager-cli --bins/);
assert.match(buildScript, /-p download-manager-native-host/);
assert.match(buildScript, /browser-extension' build:chromium/);
assert.match(buildScript, /--bundles nsis --no-sign/);
assert.match(buildScript, /download-manager-\$version-windows-x64-setup\.exe/);
assert.match(buildScript, /Get-FileHash[^\n]+SHA256/);

assert.match(registrationScript, /ValidateSet\('All', 'Chromium', 'Firefox'\)/);
assert.match(registrationScript, /Google\\Chrome\\NativeMessagingHosts/);
assert.match(registrationScript, /Microsoft\\Edge\\NativeMessagingHosts/);
assert.match(browserInstructions, /chrome:\/\/extensions/);
assert.match(browserInstructions, /edge:\/\/extensions/);
assert.match(browserInstructions, /ackbonlbpniilcfmmnfdohnhbdbajhii/);

for (const migration of [1, 2, 3, 4, 5]) {
  assert.match(
    storage,
    new RegExp(`include_str!\\("\\.\\./migrations/00${migration}_[^"]+"\\)`),
  );
}

console.log("Private-use Tauri/NSIS packaging contract passed.");
