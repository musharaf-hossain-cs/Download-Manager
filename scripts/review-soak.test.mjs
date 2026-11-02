import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";

import { reviewSoakBytes } from "./review-soak.mjs";

function passedReport() {
  const campaigns = [];
  for (let index = 0; index < 100; index += 1) {
    campaigns.push(
      { campaign: "process-worker-network", ok: true, exit_code: 0 },
      { campaign: "disk-db-update", ok: true, exit_code: 0 },
    );
  }
  return {
    schema_version: 1,
    profile: "validation",
    status: "passed",
    started_at: "2026-11-03T00:00:00.000Z",
    finished_at: "2026-11-03T01:00:00.000Z",
    duration_target_minutes: 60,
    elapsed_minutes: 60,
    mixed_job_limit: 10_000,
    verified_mixed_jobs: 10_000,
    completion_reason: "mixed_job_limit",
    campaigns,
  };
}

const passBytes = Buffer.from(JSON.stringify(passedReport()));
const pass = reviewSoakBytes(passBytes);
assert.equal(pass.decision, "pass");
assert.equal(pass.continuations.length, 0);
assert.equal(
  pass.source_sha256,
  createHash("sha256").update(passBytes).digest("hex"),
);

const malformed = passedReport();
malformed.finished_at = "not-a-timestamp";
malformed.verified_mixed_jobs = 50;
delete malformed.schema_version;
const invalid = reviewSoakBytes(JSON.stringify(malformed));
assert.equal(invalid.decision, "invalid");
assert.deepEqual(invalid.errors, [...invalid.errors].sort());
assert.ok(invalid.errors.includes("schema_version must be 1"));
assert.ok(invalid.errors.includes("finished_at is invalid"));
assert.ok(
  invalid.errors.includes(
    "verified mixed jobs do not match successful campaigns",
  ),
);

const repeatedFailure = {
  schema_version: 1,
  profile: "validation",
  status: "failed",
  started_at: "2026-11-03T00:00:00.000Z",
  finished_at: "2026-11-03T00:05:00.000Z",
  duration_target_minutes: 60,
  mixed_job_limit: 10_000,
  verified_mixed_jobs: 50,
  failure: {
    kind: "campaign_failure",
    campaign: "disk-db-update",
    exit_code: 17,
  },
  campaigns: [
    { campaign: "process-worker-network", ok: true, exit_code: 0 },
    { campaign: "disk-db-update", ok: false, exit_code: 17 },
    { campaign: "disk-db-update", ok: false, exit_code: 17 },
    {
      campaign: "process-worker-network",
      ok: false,
      exit_code: null,
      timed_out: true,
    },
  ],
};
const failure = reviewSoakBytes(JSON.stringify(repeatedFailure));
assert.equal(failure.decision, "continuation_required");
assert.equal(failure.continuations.length, 2);
assert.deepEqual(
  failure.continuations.map((continuation) => continuation.root_cause),
  [
    "disk-db-update:campaign_failure:17",
    "process-worker-network:campaign_deadlock_timeout:none",
  ],
);

console.log(
  "soak review contract passed: clean evidence, malformed rejection, root-cause deduplication",
);
