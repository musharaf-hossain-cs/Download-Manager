import { readFile } from "node:fs/promises";

const document = await readFile("docs/THREAT_MODEL.md", "utf8");
const failures = [];
const requiredThreats = [
  "Malicious webpage",
  "Malicious download server",
  "Compromised extension message",
  "Local unprivileged process",
  "Path traversal",
  "Update tampering",
  "Media parser exploit",
];

for (const threat of requiredThreats) {
  if (!document.toLocaleLowerCase("en-US").includes(threat.toLowerCase())) {
    failures.push(`missing SEC-001 threat: ${threat}`);
  }
}

const threatRows = document
  .split(/\r?\n/u)
  .filter((line) => /^\| TM-\d{2} \|/u.test(line));

if (threatRows.length !== requiredThreats.length) {
  failures.push(
    `expected ${requiredThreats.length} threat rows, found ${threatRows.length}`,
  );
}

for (const row of threatRows) {
  const cells = row
    .split("|")
    .slice(1, -1)
    .map((cell) => cell.trim());
  if (cells.length !== 6 || cells.slice(1).some((cell) => cell.length < 8)) {
    failures.push(`incomplete threat register row: ${row}`);
  }
  if (/\b(?:TBD|TODO|unknown owner)\b/iu.test(row)) {
    failures.push(`placeholder remains in threat register row: ${row}`);
  }
}

for (const heading of [
  "## Security objectives",
  "## Trust boundaries and data flow",
  "## Threat register",
  "## Release review procedure",
  "## Assumptions and exclusions",
]) {
  if (!document.includes(heading)) {
    failures.push(`missing threat-model section: ${heading}`);
  }
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  console.log(
    `threat model covers ${threatRows.length} named threats with mitigations, residual risks, and owners`,
  );
}
