import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const TEN_GIB = 10 * 1024 * 1024 * 1024;
const MINIMUM_PAIRS = 10;
const SCENARIO = "ac01-range-10gib-auto";

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function requireExactKeys(value, expected, label) {
  requireValue(
    value && typeof value === "object",
    `${label} must be an object`,
  );
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  requireValue(
    JSON.stringify(actual) === JSON.stringify(wanted),
    `${label} keys must be exactly: ${wanted.join(", ")}`,
  );
}

function requireSha256(value, label) {
  requireValue(
    typeof value === "string" && /^[0-9a-f]{64}$/u.test(value),
    `${label} must be a lowercase SHA-256`,
  );
}

export function validateAc01Evidence(manifest, plan, report) {
  requireExactKeys(
    manifest,
    ["schema_version", "acceptance_id", "fixture", "execution"],
    "manifest",
  );
  requireValue(manifest.schema_version === 1, "manifest schema must be 1");
  requireValue(
    manifest.acceptance_id === "AC-01",
    "manifest must identify AC-01",
  );
  requireExactKeys(
    manifest.fixture,
    ["controlled", "range_capable", "content_length_bytes", "sha256"],
    "fixture",
  );
  requireValue(
    manifest.fixture.controlled === true,
    "fixture must be controlled",
  );
  requireValue(
    manifest.fixture.range_capable === true,
    "fixture must be range-capable",
  );
  requireValue(
    manifest.fixture.content_length_bytes === TEN_GIB,
    `fixture must contain exactly ${TEN_GIB} bytes`,
  );
  requireSha256(manifest.fixture.sha256, "fixture sha256");

  requireExactKeys(
    manifest.execution,
    [
      "same_physical_machine",
      "background_traffic_controlled",
      "baseline_installed",
      "baseline_channel",
      "candidate_installed",
      "candidate_mode",
    ],
    "execution",
  );
  requireValue(
    manifest.execution.same_physical_machine === true,
    "both applications must run on the same physical machine",
  );
  requireValue(
    manifest.execution.background_traffic_controlled === true,
    "background traffic must be controlled",
  );
  requireValue(
    manifest.execution.baseline_installed === true &&
      manifest.execution.baseline_channel === "stable",
    "the baseline must be an installed stable IDM build",
  );
  requireValue(
    manifest.execution.candidate_installed === true &&
      manifest.execution.candidate_mode === "Auto",
    "the installed candidate must use Auto mode",
  );

  requireValue(plan.schema_version === 1, "benchmark plan schema must be 1");
  requireValue(plan.scenario === SCENARIO, `scenario must be ${SCENARIO}`);
  requireValue(
    plan.pairs >= MINIMUM_PAIRS,
    "benchmark needs at least 10 pairs",
  );
  requireValue(
    typeof plan.source === "string" && /^https?:\/\//u.test(plan.source),
    "benchmark source must be HTTP(S)",
  );
  requireValue(
    plan.source.includes("{pair}") && plan.source.includes("{seed}"),
    "benchmark source must vary by pair and seed to avoid cache reuse",
  );
  requireValue(
    plan.baseline?.name === "IDM" &&
      plan.candidate?.name === "Download Manager",
    "plan applications must be IDM and Download Manager",
  );
  const candidateArguments = plan.candidate?.download_command?.arguments;
  requireValue(
    Array.isArray(candidateArguments),
    "candidate arguments are required",
  );
  requireValue(
    candidateArguments.some(
      (argument, index) =>
        /^--mode$/u.test(argument) &&
        /^auto$/iu.test(candidateArguments[index + 1] ?? ""),
    ),
    "candidate wrapper must receive --mode Auto",
  );

  requireValue(
    report.schema_version === 1,
    "benchmark report schema must be 1",
  );
  requireValue(
    report.scenario === SCENARIO,
    "report scenario does not match AC-01",
  );
  requireValue(report.seed === plan.seed, "report seed does not match plan");
  requireValue(
    report.protocol === plan.protocol,
    "report protocol does not match plan",
  );
  requireValue(
    report.protocol === "HTTP/1.1" || report.protocol === "HTTP/2",
    "AC-01 report protocol must be HTTP/1.1 or HTTP/2",
  );
  requireValue(report.baseline?.name === "IDM", "report baseline must be IDM");
  requireValue(
    report.candidate?.name === "Download Manager",
    "report candidate must be Download Manager",
  );
  requireValue(
    typeof report.baseline.version === "string" &&
      report.baseline.version.trim(),
    "exact IDM version is required",
  );
  requireValue(
    typeof report.candidate.version === "string" &&
      report.candidate.version.trim(),
    "exact candidate version is required",
  );
  requireValue(
    Array.isArray(report.samples) && report.samples.length === plan.pairs,
    "report sample count must match the plan",
  );

  const pairs = new Set();
  for (const sample of report.samples) {
    requireValue(!pairs.has(sample.pair), `duplicate pair ${sample.pair}`);
    pairs.add(sample.pair);
    requireValue(
      Number.isInteger(sample.pair) &&
        sample.pair >= 1 &&
        sample.pair <= plan.pairs,
      `invalid pair number ${sample.pair}`,
    );
    const expectedSource = plan.source
      .replaceAll("{pair}", String(sample.pair))
      .replaceAll("{seed}", String(plan.seed));
    requireValue(
      sample.source === expectedSource,
      `pair ${sample.pair} source does not match the seeded plan`,
    );
    for (const role of ["baseline", "candidate"]) {
      const run = sample[role];
      requireValue(
        run?.role === role,
        `pair ${sample.pair} ${role} role is invalid`,
      );
      requireValue(
        run?.bytes === TEN_GIB,
        `pair ${sample.pair} ${role} did not complete 10 GiB`,
      );
      requireValue(
        run.sha256 === manifest.fixture.sha256,
        `pair ${sample.pair} ${role} hash does not match the fixture`,
      );
    }
    requireValue(
      sample.output_hash_matches === true,
      `pair ${sample.pair} did not match output hashes`,
    );
  }
  requireValue(pairs.size === plan.pairs, "report pair coverage is incomplete");
  requireValue(
    report.summary?.all_output_hashes_match === true,
    "summary did not confirm every output hash",
  );
  const baselineMedian =
    report.summary?.baseline_goodput_bytes_per_second?.median;
  const candidateMedian =
    report.summary?.candidate_goodput_bytes_per_second?.median;
  requireValue(
    Number.isFinite(baselineMedian) && baselineMedian > 0,
    "baseline median goodput must be positive",
  );
  requireValue(
    Number.isFinite(candidateMedian) && candidateMedian / baselineMedian >= 1,
    "candidate median goodput ratio must be at least 1.00",
  );
  requireValue(
    report.summary?.paired_goodput_change_percent?.median >= -3,
    "paired median regression exceeds 3 percent",
  );

  return {
    acceptance_id: "AC-01",
    status: "passed",
    pairs: report.samples.length,
    bytes_per_run: TEN_GIB,
    candidate_mode: "Auto",
    output_sha256: manifest.fixture.sha256,
    median_goodput_ratio: candidateMedian / baselineMedian,
  };
}

async function runCli() {
  const [manifestPath, planPath, reportPath] = process.argv.slice(2);
  if (!manifestPath || !planPath || !reportPath) {
    throw new Error(
      "usage: node scripts/validate-ac01-evidence.mjs <manifest.json> <plan.json> <report.json>",
    );
  }
  const [manifest, plan, report] = await Promise.all(
    [manifestPath, planPath, reportPath].map(async (path) =>
      JSON.parse(await readFile(path, "utf8")),
    ),
  );
  console.log(
    JSON.stringify(validateAc01Evidence(manifest, plan, report), null, 2),
  );
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  await runCli();
}
