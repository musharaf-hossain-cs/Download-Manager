import assert from "node:assert/strict";
import { validateAc01Evidence } from "./validate-ac01-evidence.mjs";

const bytes = 10 * 1024 * 1024 * 1024;
const sha256 = "a".repeat(64);

function fixture() {
  const manifest = {
    schema_version: 1,
    acceptance_id: "AC-01",
    fixture: {
      controlled: true,
      range_capable: true,
      content_length_bytes: bytes,
      sha256,
    },
    execution: {
      same_physical_machine: true,
      background_traffic_controlled: true,
      baseline_installed: true,
      baseline_channel: "stable",
      candidate_installed: true,
      candidate_mode: "Auto",
    },
  };
  const plan = {
    schema_version: 1,
    scenario: "ac01-range-10gib-auto",
    protocol: "HTTP/2",
    seed: 42,
    pairs: 10,
    source: "https://fixture.invalid/10gib?pair={pair}&seed={seed}",
    output_directory: "C:/release-lab/output",
    baseline: {
      name: "IDM",
      download_command: { arguments: ["--url", "{url}"] },
    },
    candidate: {
      name: "Download Manager",
      download_command: { arguments: ["--mode", "Auto", "--url", "{url}"] },
    },
  };
  const samples = Array.from({ length: 10 }, (_, index) => ({
    pair: index + 1,
    first: index % 2 === 0 ? "baseline" : "candidate",
    source: plan.source
      .replace("{pair}", String(index + 1))
      .replace("{seed}", String(plan.seed)),
    baseline: { role: "baseline", bytes, sha256 },
    candidate: { role: "candidate", bytes, sha256 },
    output_hash_matches: true,
  }));
  const report = {
    schema_version: 1,
    scenario: plan.scenario,
    protocol: plan.protocol,
    seed: plan.seed,
    baseline: { name: "IDM", version: "IDM 6.42.41" },
    candidate: { name: "Download Manager", version: "0.1.0" },
    samples,
    summary: {
      all_output_hashes_match: true,
      baseline_goodput_bytes_per_second: { median: 1_000_000 },
      candidate_goodput_bytes_per_second: { median: 1_010_000 },
      paired_goodput_change_percent: { median: 1 },
    },
  };
  return { manifest, plan, report };
}

const valid = fixture();
assert.deepEqual(validateAc01Evidence(...Object.values(valid)), {
  acceptance_id: "AC-01",
  status: "passed",
  pairs: 10,
  bytes_per_run: bytes,
  candidate_mode: "Auto",
  output_sha256: sha256,
  median_goodput_ratio: 1.01,
});

for (const mutate of [
  (value) => (value.manifest.fixture.range_capable = false),
  (value) => (value.manifest.execution.candidate_mode = "Manual"),
  (value) => (value.plan.candidate.download_command.arguments[1] = "Manual"),
  (value) =>
    (value.plan.candidate.download_command.arguments = [
      "Auto",
      "--mode",
      "Manual",
    ]),
  (value) => (value.report.seed = 43),
  (value) => (value.report.samples[0].source = "https://wrong.invalid/file"),
  (value) => (value.report.samples[0].candidate.role = "baseline"),
  (value) => (value.report.samples[0].candidate.bytes = bytes - 1),
  (value) => (value.report.samples[0].candidate.sha256 = "b".repeat(64)),
  (value) =>
    (value.report.summary.candidate_goodput_bytes_per_second.median = 999_999),
]) {
  const invalid = fixture();
  mutate(invalid);
  assert.throws(() => validateAc01Evidence(...Object.values(invalid)));
}

console.log("AC-01 evidence validator checks passed");
