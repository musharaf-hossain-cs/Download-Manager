import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const workflow = await readFile(
  ".github/workflows/platform-compatibility.yml",
  "utf8",
);
const runner = await readFile(
  "scripts/test-platform-compatibility.ps1",
  "utf8",
);
const builder = await readFile("scripts/build-windows-installer.ps1", "utf8");

for (const profile of [
  "windows-10-22h2-x64",
  "windows-11-23h2-x64",
  "windows-11-24h2-x64",
  "windows-11-25h2-x64",
  "windows-11-26h2-x64",
  "windows-11-26h1-arm64-x64-emulation",
]) {
  assert.match(workflow, new RegExp(`profile: ${profile}`, "u"));
  assert.match(runner, new RegExp(`'${profile}'`, "u"));
}

assert.match(workflow, /runs-on: \$\{\{ fromJSON\(matrix\.runner\) \}\}/u);
assert.match(workflow, /"x64-emulation"/u);
assert.match(workflow, /if: \$\{\{ always\(\) \}\}/u);
assert.match(runner, /build = 19045/u);
assert.match(runner, /build = 22631/u);
assert.match(runner, /build = 26100/u);
assert.match(runner, /build = 26200/u);
assert.match(runner, /build = 26300/u);
assert.match(runner, /build = 28000/u);
assert.match(runner, /actualBuild -ne \$expected\.build/u);
assert.match(runner, /actualArchitecture -cne \$expected\.os_architecture/u);
assert.match(
  runner,
  /\$status = if \(\$EnvironmentOnly\) \{ 'environment-only' \} else \{ 'passed' \}/u,
);
assert.match(
  runner,
  /installed_product_smoke = \(-not \$EnvironmentOnly -and \$status -eq 'passed'\)/u,
);
assert.match(runner, /test-windows-installer\.ps1/u);
assert.match(builder, /TargetTriple = 'x86_64-pc-windows-msvc'/u);
assert.match(builder, /\$crossCompiling = \$hostTriple -cne \$TargetTriple/u);
assert.match(builder, /@\('--target', \$TargetTriple\)/u);
assert.match(builder, /cargo @buildArguments/u);

console.log("platform compatibility contract checks passed");
