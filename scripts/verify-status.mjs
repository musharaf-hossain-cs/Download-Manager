import { readFile } from "node:fs/promises";

const plan = await readFile("PROGRESSIVE_IMPLEMENTATION_PLAN.md", "utf8");
const status = await readFile("IMPLEMENTATION_STATUS.md", "utf8");

const planned = [...plan.matchAll(/^\| (S\d{3}) \| Q[12] \|/gmu)].map(
  (match) => match[1],
);
const rows = [
  ...status.matchAll(/^\| (S\d{3}) \| (TODO|IN PROGRESS|DONE|BLOCKED) \|/gmu),
];
const counts = new Map();

for (const match of rows) {
  const id = match[1];
  counts.set(id, (counts.get(id) ?? 0) + 1);
}

const failures = [];
for (const id of planned) {
  if (counts.get(id) !== 1) {
    failures.push(
      `${id} has ${counts.get(id) ?? 0} status rows (expected one)`,
    );
  }
}

for (const id of counts.keys()) {
  if (!planned.includes(id)) {
    failures.push(`${id} is not present in the implementation plan`);
  }
}

if (new Set(planned).size !== planned.length) {
  failures.push("the implementation plan contains duplicate slice IDs");
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  console.log(`status verification passed (${planned.length} slices)`);
}
