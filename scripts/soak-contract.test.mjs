import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

import { runSoak, SOAK_CAMPAIGNS, SOAK_PROFILES } from "./soak.mjs";

assert.equal(SOAK_PROFILES.release_24h.durationMinutes, 24 * 60);
assert.equal(SOAK_PROFILES.validation.durationMinutes, 60);
assert.equal(SOAK_PROFILES.release_24h.mixedJobLimit, 10_000);

let clock = Date.parse("2026-11-03T00:00:00.000Z");
const healthy = await runSoak({
  profile: "validation",
  durationMs: 60 * 60_000,
  mixedJobLimit: 10_000,
  executeCampaign: async (campaign) => ({
    campaign: campaign.id,
    ok: true,
    exit_code: 0,
    output: "",
  }),
  now: () => clock,
  sleep: async (milliseconds) => {
    clock += milliseconds;
  },
});
assert.equal(healthy.status, "passed");
assert.equal(healthy.completion_reason, "mixed_job_limit");
assert.equal(healthy.elapsed_minutes, 60);
assert.equal(healthy.verified_mixed_jobs, 10_000);

clock = Date.parse("2026-11-03T00:00:00.000Z");
let executions = 0;
const injectedFailure = await runSoak({
  profile: "validation",
  durationMs: 60 * 60_000,
  mixedJobLimit: 10_000,
  executeCampaign: async (campaign) => {
    executions += 1;
    return executions === 2
      ? {
          campaign: campaign.id,
          ok: false,
          exit_code: 17,
          output: "deliberate contract-test failure",
        }
      : { campaign: campaign.id, ok: true, exit_code: 0, output: "" };
  },
  now: () => clock,
  sleep: async (milliseconds) => {
    clock += milliseconds;
  },
});
assert.equal(injectedFailure.status, "failed");
assert.equal(injectedFailure.failure.kind, "campaign_failure");
assert.equal(injectedFailure.failure.exit_code, 17);
assert.equal(injectedFailure.verified_mixed_jobs, 50);

clock = Date.parse("2026-11-03T00:00:00.000Z");
const deadlock = await runSoak({
  profile: "validation",
  durationMs: 60 * 60_000,
  mixedJobLimit: 10_000,
  campaigns: [SOAK_CAMPAIGNS[0]],
  executeCampaign: async (campaign) => ({
    campaign: campaign.id,
    ok: false,
    exit_code: null,
    timed_out: true,
    output: "campaign timed out",
  }),
  now: () => clock,
  sleep: async (milliseconds) => {
    clock += milliseconds;
  },
});
assert.equal(deadlock.status, "failed");
assert.equal(deadlock.failure.kind, "campaign_deadlock_timeout");
assert.equal(deadlock.verified_mixed_jobs, 0);

const workflow = await readFile(".github/workflows/soak.yml", "utf8");
assert.match(workflow, /schedule:\s*\n\s*- cron:/u);
assert.match(workflow, /- release_24h/u);
assert.match(workflow, /default:\s*validation/u);
assert.match(workflow, /timeout-minutes:\s*1500/u);
assert.doesNotMatch(workflow, /inject.?failure/iu);

console.log(
  "soak contract passed: 24-hour schedule, 60-minute validation, failure and timeout detection, healthy committed default",
);
