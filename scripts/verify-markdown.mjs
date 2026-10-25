import { access, readFile } from "node:fs/promises";
import path from "node:path";

const root = process.cwd();
const requiredFiles = [
  "README.md",
  "REQUIREMENTS.md",
  "PROGRESSIVE_IMPLEMENTATION_PLAN.md",
];

const failures = [];

for (const relativePath of requiredFiles) {
  try {
    await access(path.join(root, relativePath));
  } catch {
    failures.push(`missing required file: ${relativePath}`);
  }
}

const requirements = await readFile(path.join(root, "REQUIREMENTS.md"), "utf8");
const requirementMatches = [
  ...requirements.matchAll(/^\| ([A-Z][A-Z-]+-\d{2,3}) \|/gmu),
].map((match) => match[1]);
const requirementCounts = new Map();

for (const id of requirementMatches) {
  requirementCounts.set(id, (requirementCounts.get(id) ?? 0) + 1);
}

for (const [id, count] of requirementCounts) {
  if (count !== 1) {
    failures.push(`requirement ${id} appears ${count} times (expected once)`);
  }
}

if (requirementCounts.size === 0) {
  failures.push("no requirement IDs found");
}

for (const relativePath of requiredFiles) {
  const contents = await readFile(path.join(root, relativePath), "utf8");
  for (const match of contents.matchAll(/\[[^\]]*\]\(([^)]+)\)/g)) {
    const destination = match[1]?.split("#", 1)[0];
    if (
      !destination ||
      /^(?:https?:|mailto:)/u.test(destination) ||
      destination.startsWith("#")
    ) {
      continue;
    }

    const decoded = decodeURIComponent(destination);
    const target = path.resolve(
      path.dirname(path.join(root, relativePath)),
      decoded,
    );
    try {
      await access(target);
    } catch {
      failures.push(
        `${relativePath} links to missing local path: ${destination}`,
      );
    }
  }
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  console.log(
    `documentation verification passed (${requirementCounts.size} unique requirement IDs)`,
  );
}
