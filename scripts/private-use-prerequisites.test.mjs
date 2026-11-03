import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const script = await readFile(
  "scripts/check-private-use-prerequisites.ps1",
  "utf8",
);

for (const required of [
  "rust-toolchain.toml",
  ".node-version",
  "package.json",
  "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
  "Hostx64\\x64\\link.exe",
  "Windows Kits\\10",
  "um\\Windows.h",
  "um\\x64\\kernel32.lib",
  "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
  "Tauri 2 Windows prerequisites",
  "a global Tauri CLI is not required",
  "%LOCALAPPDATA%\\DownloadManager\\State",
  "%LOCALAPPDATA%\\DownloadManager\\Logs",
  "%LOCALAPPDATA%\\DownloadManager\\State\\partials",
  "%LOCALAPPDATA%\\DownloadManager\\ipc",
  "%USERPROFILE%\\Downloads\\DownloadManager-Test",
]) {
  assert.ok(
    script.includes(required),
    `missing prerequisite contract: ${required}`,
  );
}

assert.match(script, /@\('build', '--workspace', '--locked'\)/u);
assert.match(script, /'pnpm' @\('build'\)/u);
assert.match(script, /'pnpm' @\('typecheck'\)/u);
assert.match(script, /rustup toolchain install \$rustVersion/u);
assert.match(script, /corepack prepare pnpm@\$pnpmVersion --activate/u);
assert.match(script, /visualstudio\.microsoft\.com\/downloads/u);
assert.match(script, /developer\.microsoft\.com\/microsoft-edge\/webview2/u);

for (const mutatingOperation of [
  /New-Item/u,
  /Set-ItemProperty/u,
  /New-ItemProperty/u,
  /Start-Process/u,
  /winget\s+install/iu,
]) {
  assert.doesNotMatch(script, mutatingOperation);
}

console.log("private-use prerequisite contract checks passed");
