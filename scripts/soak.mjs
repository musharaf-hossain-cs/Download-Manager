import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const SOAK_PROFILES = Object.freeze({
  validation: Object.freeze({
    durationMinutes: 60,
    mixedJobLimit: 10_000,
  }),
  release_24h: Object.freeze({
    durationMinutes: 24 * 60,
    mixedJobLimit: 10_000,
  }),
});

export const SOAK_CAMPAIGNS = Object.freeze([
  Object.freeze({
    id: "process-worker-network",
    completedJobs: 50,
    args: Object.freeze([
      "test",
      "--locked",
      "-p",
      "download-manager-engine",
      "--test",
      "fault_campaign_s175",
      "fifty_seeded_process_worker_network_faults_never_mark_corrupt_output_complete",
      "--",
      "--exact",
    ]),
  }),
  Object.freeze({
    id: "disk-db-update",
    completedJobs: 50,
    args: Object.freeze([
      "test",
      "--locked",
      "-p",
      "download-manager-engine",
      "--test",
      "disk_db_update_interruptions_s176",
      "fifty_seeded_disk_db_and_update_interruptions_preserve_invariants",
      "--",
      "--exact",
    ]),
  }),
]);

const CAMPAIGN_TIMEOUT_MS = 20 * 60 * 1_000;
const MAX_CAPTURED_OUTPUT = 16 * 1_024;

function appendBounded(current, chunk) {
  const combined = current + chunk;
  return combined.length > MAX_CAPTURED_OUTPUT
    ? combined.slice(-MAX_CAPTURED_OUTPUT)
    : combined;
}

function terminateProcessTree(child) {
  if (process.platform === "win32" && child.pid) {
    spawn("taskkill", ["/pid", String(child.pid), "/t", "/f"], {
      shell: false,
      stdio: "ignore",
      windowsHide: true,
    });
  } else {
    child.kill("SIGKILL");
  }
}

export function executeCargoCampaign(campaign, options = {}) {
  const timeoutMs = options.timeoutMs ?? CAMPAIGN_TIMEOUT_MS;

  return new Promise((resolveCampaign) => {
    const startedAt = Date.now();
    const child = spawn("cargo", campaign.args, {
      cwd: options.cwd,
      env: process.env,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    let timedOut = false;
    let settled = false;

    const finish = (result) => {
      if (settled) return;
      settled = true;
      globalThis.clearTimeout(timer);
      resolveCampaign({
        campaign: campaign.id,
        duration_ms: Date.now() - startedAt,
        ...result,
        output: output.trim(),
      });
    };

    child.stdout.on("data", (chunk) => {
      output = appendBounded(output, chunk.toString("utf8"));
    });
    child.stderr.on("data", (chunk) => {
      output = appendBounded(output, chunk.toString("utf8"));
    });
    child.on("error", (error) => {
      finish({ ok: false, exit_code: null, error: error.message });
    });
    child.on("close", (code, signal) => {
      finish({
        ok: code === 0 && !timedOut,
        exit_code: code,
        signal,
        timed_out: timedOut,
      });
    });

    const timer = globalThis.setTimeout(() => {
      timedOut = true;
      terminateProcessTree(child);
    }, timeoutMs);
    timer.unref();
  });
}

export async function runSoak({
  profile,
  durationMs,
  mixedJobLimit,
  campaigns = SOAK_CAMPAIGNS,
  executeCampaign,
  now = Date.now,
  sleep = (milliseconds) =>
    new Promise((resolveSleep) =>
      globalThis.setTimeout(resolveSleep, milliseconds),
    ),
  onProgress = () => {},
}) {
  const startedAtMs = now();
  const deadlineMs = startedAtMs + durationMs;
  const campaignResults = [];
  let mixedJobs = 0;

  while (now() < deadlineMs && mixedJobs < mixedJobLimit) {
    for (const campaign of campaigns) {
      if (now() >= deadlineMs || mixedJobs >= mixedJobLimit) break;
      const result = await executeCampaign(campaign);
      campaignResults.push(result);
      onProgress({ campaign: campaign.id, result, mixedJobs });

      if (!result.ok) {
        return {
          schema_version: 1,
          profile,
          status: "failed",
          started_at: new Date(startedAtMs).toISOString(),
          finished_at: new Date(now()).toISOString(),
          duration_target_minutes: durationMs / 60_000,
          mixed_job_limit: mixedJobLimit,
          verified_mixed_jobs: mixedJobs,
          failure: {
            kind: result.timed_out
              ? "campaign_deadlock_timeout"
              : "campaign_failure",
            campaign: campaign.id,
            exit_code: result.exit_code ?? null,
            output: result.output ?? "",
          },
          campaigns: campaignResults,
        };
      }

      mixedJobs += campaign.completedJobs;
      const pacedUntilMs =
        startedAtMs + (durationMs * mixedJobs) / mixedJobLimit;
      if (pacedUntilMs > now()) await sleep(pacedUntilMs - now());
    }
  }

  const finishedAtMs = now();
  return {
    schema_version: 1,
    profile,
    status: "passed",
    started_at: new Date(startedAtMs).toISOString(),
    finished_at: new Date(finishedAtMs).toISOString(),
    duration_target_minutes: durationMs / 60_000,
    elapsed_minutes: (finishedAtMs - startedAtMs) / 60_000,
    mixed_job_limit: mixedJobLimit,
    verified_mixed_jobs: mixedJobs,
    completion_reason:
      mixedJobs >= mixedJobLimit ? "mixed_job_limit" : "duration_elapsed",
    campaigns: campaignResults,
  };
}

function parseArguments(args) {
  const profileIndex = args.indexOf("--profile");
  const outputIndex = args.indexOf("--output");
  const profile = profileIndex >= 0 ? args[profileIndex + 1] : undefined;
  const output = outputIndex >= 0 ? args[outputIndex + 1] : undefined;

  if (!Object.hasOwn(SOAK_PROFILES, profile)) {
    throw new Error("--profile must be validation or release_24h");
  }
  if (!output) throw new Error("--output is required");
  return { profile, output };
}

async function main() {
  const { profile, output } = parseArguments(process.argv.slice(2));
  const settings = SOAK_PROFILES[profile];
  const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const report = await runSoak({
    profile,
    durationMs: settings.durationMinutes * 60_000,
    mixedJobLimit: settings.mixedJobLimit,
    executeCampaign: (campaign) =>
      executeCargoCampaign(campaign, { cwd: repositoryRoot }),
    onProgress: ({ campaign, result, mixedJobs }) => {
      console.log(
        `${new Date().toISOString()} ${campaign}: ${result.ok ? "passed" : "failed"}; ${mixedJobs} mixed jobs previously verified`,
      );
    },
  });
  const outputPath = resolve(repositoryRoot, output);
  await mkdir(dirname(outputPath), { recursive: true });
  await writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  console.log(`Soak report: ${outputPath}`);
  if (report.status !== "passed") process.exitCode = 1;
}

const scriptPath = fileURLToPath(import.meta.url);
if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  await main();
}
