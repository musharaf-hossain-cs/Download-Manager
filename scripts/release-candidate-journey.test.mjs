import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const runner = await readFile("scripts/release-candidate-journey.ps1", "utf8");
const installer = await readFile("scripts/test-windows-installer.ps1", "utf8");
const workflow = await readFile(
  ".github/workflows/release-candidate.yml",
  "utf8",
);
const packageJson = JSON.parse(await readFile("package.json", "utf8"));
const evidence = JSON.parse(
  await readFile("docs/evidence/s182-release-candidate-journey.json", "utf8"),
);

const orderedStages = [
  "install",
  "capture",
  "resume",
  "licensed_media",
  "update",
  "uninstall",
];
let previous = -1;
for (const stage of orderedStages) {
  const position = runner.indexOf(`'${stage}'`);
  assert.ok(
    position > previous,
    `${stage} must follow the prior journey stage`,
  );
  previous = position;
}

for (const required of [
  "test-windows-installer.ps1",
  "test/link-handoff.test.ts",
  "same_object_resumes_only_the_missing_range",
  "media_corpus_s173",
  "media_corpus_s174",
  "update_activation::tests",
  "uninstall.ps1",
]) {
  assert.ok(runner.includes(required), `runner is missing ${required}`);
}
assert.match(installer, /\[scriptblock\]\s*\$BeforeUninstall/u);
assert.match(installer, /& \$BeforeUninstall/u);
assert.equal(
  packageJson.scripts["test:release-candidate"],
  "pwsh -File scripts/release-candidate-journey.ps1",
);
assert.match(workflow, /runs-on:\s*windows-latest/u);
assert.match(workflow, /pnpm test:release-candidate/u);
assert.match(workflow, /actions\/upload-artifact@/u);
assert.match(
  runner,
  /native host AddDownload path uses authenticated current-user backend IPC/u,
);

assert.equal(evidence.status, "passed");
assert.deepEqual(
  evidence.stages.map(({ name }) => name),
  orderedStages,
);
assert.equal(evidence.environment.supported_windows_vm_matrix_claimed, false);
const boundaries = evidence.evidence_boundaries.join(" ");
for (const boundary of [
  "No installed browser capture",
  "native host AddDownload path remains not_ready",
  "No packaged Windows update activation adapter",
  "not a Windows 10/11 supported-OS clean-VM matrix claim",
  "S180 requirement blockers",
]) {
  assert.ok(
    boundaries.includes(boundary),
    `missing evidence boundary: ${boundary}`,
  );
}

console.log(
  "S182 contract passed: ordered composed gate, required focused commands, hosted-Windows entry, and explicit non-claims",
);
