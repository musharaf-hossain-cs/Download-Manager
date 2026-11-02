import { readFile } from "node:fs/promises";

const requirements = await readFile("REQUIREMENTS.md", "utf8");
const traceability = await readFile("docs/TRACEABILITY.md", "utf8");

const REQUIRED_REQUIREMENT_COUNT = 170;
const REQUIRED_ACCEPTANCE_COUNT = 16;
const allowedStatuses = new Set(["Implemented", "Verified", "Blocked"]);
const placeholder = /^(?:[-—]|none|n\/a|tbd|todo)$/iu;

const expected = new Map();
for (const match of requirements.matchAll(
  /^\|\s*([A-Z][A-Z0-9-]*-\d{2,3})\s*\|\s*(P[01])\s*\|/gmu,
)) {
  addExpected(match[1], match[2], "requirement");
}

const explicitRequirementCount = expected.size;
for (const match of requirements.matchAll(/^\|\s*(AC-\d{2})\s*\|/gmu)) {
  addExpected(match[1], "P0", "v1 acceptance criterion");
}

const failures = [];
if (explicitRequirementCount !== REQUIRED_REQUIREMENT_COUNT) {
  failures.push(
    `found ${explicitRequirementCount} explicit P0/P1 requirements (expected ${REQUIRED_REQUIREMENT_COUNT})`,
  );
}
if (expected.size - explicitRequirementCount !== REQUIRED_ACCEPTANCE_COUNT) {
  failures.push(
    `found ${expected.size - explicitRequirementCount} v1 acceptance criteria (expected ${REQUIRED_ACCEPTANCE_COUNT})`,
  );
}

const rows = new Map();
for (const line of traceability.split(/\r?\n/u)) {
  if (!line.startsWith("| `")) continue;
  const match = line.match(
    /^\|\s*`([^`]+)`\s*\|\s*(P[012])\s*\|\s*([^|]+?)\s*\|\s*([^|]+?)\s*\|\s*([^|]+?)\s*\|\s*([^|]+?)\s*\|\s*$/u,
  );
  if (!match) {
    failures.push(`malformed traceability row: ${line}`);
    continue;
  }

  const [, id, priority, slices, status, evidence, notes] = match;
  if (rows.has(id)) {
    failures.push(`traceability ID ${id} appears more than once`);
    continue;
  }
  rows.set(id, { priority, slices, status, evidence, notes });
}

for (const [id, row] of rows) {
  const expectedPriority = expected.get(id);
  if (expectedPriority === undefined) {
    failures.push(`traceability ID ${id} is not a mandatory requirement`);
    continue;
  }
  if (row.priority !== expectedPriority) {
    failures.push(
      `${id} has priority ${row.priority} in traceability (expected ${expectedPriority})`,
    );
  }
  if (
    !/^S\d{3}(?:[A-Z])?(?:\s*(?:,|\u2013)\s*S?\d{3}(?:[A-Z])?)*(?:\s+and\s+feature\s+slices)?$/u.test(
      row.slices,
    )
  ) {
    failures.push(`${id} has an invalid owning-slice value: ${row.slices}`);
  }
  if (!allowedStatuses.has(row.status)) {
    failures.push(`${id} has non-final audit status ${row.status}`);
  }
  validateText(id, "evidence", row.evidence);
  validateText(id, "notes", row.notes);
  if (row.status === "Blocked" && !/(?:^|\s)Blocker:/u.test(row.notes)) {
    failures.push(`${id} is Blocked but its notes do not contain Blocker:`);
  }
}

for (const id of expected.keys()) {
  if (!rows.has(id)) failures.push(`mandatory requirement ${id} is not traced`);
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  const counts = new Map();
  for (const row of rows.values()) {
    counts.set(row.status, (counts.get(row.status) ?? 0) + 1);
  }
  console.log(
    `traceability verification passed (${expected.size} mandatory entries: ${[
      "Verified",
      "Implemented",
      "Blocked",
    ]
      .map((status) => `${counts.get(status) ?? 0} ${status}`)
      .join(", ")})`,
  );
}

function addExpected(id, priority, kind) {
  if (expected.has(id)) {
    throw new Error(`${kind} ${id} appears more than once in REQUIREMENTS.md`);
  }
  expected.set(id, priority);
}

function validateText(id, field, value) {
  const normalized = value.trim();
  if (
    normalized.length === 0 ||
    placeholder.test(normalized) ||
    /(?:EXAMPLE|path\/to\/test|replace this example)/iu.test(normalized)
  ) {
    failures.push(`${id} has placeholder ${field}`);
  }
}
