import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { SOAK_CAMPAIGNS, SOAK_PROFILES } from "./soak.mjs";

const MAX_REPORT_BYTES = 4 * 1_024 * 1_024;
const MAX_CAMPAIGNS = 256;
const CAMPAIGNS_BY_ID = new Map(
  SOAK_CAMPAIGNS.map((campaign) => [campaign.id, campaign]),
);

function validTimestamp(value) {
  return typeof value === "string" && Number.isFinite(Date.parse(value));
}

function invalidReview(sourceSha256, errors) {
  return {
    schema_version: 1,
    source_sha256: sourceSha256,
    decision: "invalid",
    errors: [...new Set(errors)].sort(),
    continuations: [],
  };
}

function failureKind(result) {
  return result.timed_out === true
    ? "campaign_deadlock_timeout"
    : "campaign_failure";
}

export function reviewSoakBytes(source) {
  const bytes = Buffer.isBuffer(source) ? source : Buffer.from(source);
  const sourceSha256 = createHash("sha256").update(bytes).digest("hex");
  if (bytes.length > MAX_REPORT_BYTES) {
    return invalidReview(sourceSha256, ["report exceeds 4 MiB limit"]);
  }

  let report;
  try {
    report = JSON.parse(bytes.toString("utf8"));
  } catch {
    return invalidReview(sourceSha256, ["report is not valid JSON"]);
  }

  const errors = [];
  if (!report || typeof report !== "object" || Array.isArray(report)) {
    return invalidReview(sourceSha256, ["report root must be an object"]);
  }
  if (report.schema_version !== 1) errors.push("schema_version must be 1");
  const settings = Object.hasOwn(SOAK_PROFILES, report.profile)
    ? SOAK_PROFILES[report.profile]
    : undefined;
  if (!settings) errors.push("profile is not recognized");
  if (!validTimestamp(report.started_at)) errors.push("started_at is invalid");
  if (!validTimestamp(report.finished_at))
    errors.push("finished_at is invalid");
  if (!Array.isArray(report.campaigns)) {
    errors.push("campaigns must be an array");
  } else if (report.campaigns.length > MAX_CAMPAIGNS) {
    errors.push("campaigns exceeds 256-entry limit");
  }

  const startedAt = Date.parse(report.started_at);
  const finishedAt = Date.parse(report.finished_at);
  if (Number.isFinite(startedAt) && Number.isFinite(finishedAt)) {
    if (finishedAt < startedAt) errors.push("finished_at precedes started_at");
  }
  if (settings) {
    if (report.duration_target_minutes !== settings.durationMinutes) {
      errors.push("duration target does not match profile");
    }
    if (report.mixed_job_limit !== settings.mixedJobLimit) {
      errors.push("mixed job limit does not match profile");
    }
  }
  if (report.status !== "passed" && report.status !== "failed") {
    errors.push("status must be passed or failed");
  }

  const campaigns = Array.isArray(report.campaigns) ? report.campaigns : [];
  let accountedJobs = 0;
  const covered = new Set();
  const failures = [];
  for (const result of campaigns.slice(0, MAX_CAMPAIGNS)) {
    if (!result || typeof result !== "object") {
      errors.push("campaign result must be an object");
      continue;
    }
    const campaign = CAMPAIGNS_BY_ID.get(result.campaign);
    if (!campaign) {
      errors.push("campaign result has an unknown campaign");
      continue;
    }
    if (typeof result.ok !== "boolean") {
      errors.push("campaign result ok must be boolean");
      continue;
    }
    covered.add(campaign.id);
    if (result.ok) accountedJobs += campaign.completedJobs;
    else failures.push(result);
  }
  if (report.verified_mixed_jobs !== accountedJobs) {
    errors.push("verified mixed jobs do not match successful campaigns");
  }

  if (report.status === "passed") {
    if (failures.length > 0) errors.push("passed report contains failures");
    for (const campaign of SOAK_CAMPAIGNS) {
      if (!covered.has(campaign.id)) {
        errors.push(`passed report does not cover ${campaign.id}`);
      }
    }
    const elapsedMinutes = (finishedAt - startedAt) / 60_000;
    if (settings && elapsedMinutes < settings.durationMinutes) {
      errors.push("passed report did not reach target duration");
    }
    if (settings && accountedJobs < settings.mixedJobLimit) {
      errors.push("passed report did not reach mixed job limit");
    }
    if (
      Number.isFinite(elapsedMinutes) &&
      report.elapsed_minutes !== elapsedMinutes
    ) {
      errors.push("elapsed minutes do not match timestamps");
    }
  } else if (report.status === "failed") {
    if (failures.length === 0)
      errors.push("failed report has no failed campaign");
    if (!report.failure || typeof report.failure !== "object") {
      errors.push("failed report has no failure summary");
    } else if (
      !failures.some(
        (result) =>
          result.campaign === report.failure.campaign &&
          failureKind(result) === report.failure.kind &&
          (result.exit_code ?? null) === (report.failure.exit_code ?? null),
      )
    ) {
      errors.push("failure summary does not match a failed campaign");
    }
  }

  if (errors.length > 0) return invalidReview(sourceSha256, errors);

  const roots = new Map();
  for (const result of failures) {
    const kind = failureKind(result);
    const exitCode = result.exit_code ?? null;
    const rootCause = `${result.campaign}:${kind}:${exitCode ?? "none"}`;
    roots.set(rootCause, {
      root_cause: rootCause,
      campaign: result.campaign,
      failure_kind: kind,
      exit_code: exitCode,
    });
  }
  const continuations = [...roots.values()].sort((left, right) =>
    left.root_cause.localeCompare(right.root_cause),
  );
  return {
    schema_version: 1,
    source_sha256: sourceSha256,
    decision: report.status === "passed" ? "pass" : "continuation_required",
    source: {
      profile: report.profile,
      status: report.status,
      started_at: report.started_at,
      finished_at: report.finished_at,
      verified_mixed_jobs: report.verified_mixed_jobs,
    },
    continuations,
  };
}

function argument(args, name) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

async function main() {
  const input = argument(process.argv.slice(2), "--input");
  const output = argument(process.argv.slice(2), "--output");
  if (!input || !output) throw new Error("--input and --output are required");
  const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const inputPath = resolve(repositoryRoot, input);
  if ((await stat(inputPath)).size > MAX_REPORT_BYTES) {
    throw new Error("soak report exceeds 4 MiB limit");
  }
  const review = reviewSoakBytes(await readFile(inputPath));
  const outputPath = resolve(repositoryRoot, output);
  await mkdir(dirname(outputPath), { recursive: true });
  await writeFile(outputPath, `${JSON.stringify(review, null, 2)}\n`, "utf8");
  console.log(`Soak review: ${outputPath} (${review.decision})`);
  if (review.decision !== "pass") process.exitCode = 1;
}

const scriptPath = fileURLToPath(import.meta.url);
if (process.argv[1] && resolve(process.argv[1]) === scriptPath) await main();
