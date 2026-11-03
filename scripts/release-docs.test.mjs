import { access, readFile } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";

const root = process.cwd();
const releaseDocs = [
  "README.md",
  "docs/USER_GUIDE.md",
  "docs/PRIVACY.md",
  "docs/THIRD_PARTY_NOTICES.md",
];
const failures = [];

const contentsByFile = new Map();
for (const relativePath of releaseDocs) {
  try {
    const contents = await readFile(path.join(root, relativePath), "utf8");
    contentsByFile.set(relativePath, contents);
  } catch {
    failures.push(`missing release document: ${relativePath}`);
  }
}

const requiredSections = new Map([
  [
    "docs/USER_GUIDE.md",
    [
      "## Install and connect a browser",
      "## Add and control downloads",
      "## Media downloads",
      "## Command line",
      "## Troubleshooting",
      "## Uninstall",
    ],
  ],
  [
    "docs/PRIVACY.md",
    [
      "## Data processed locally",
      "## Network activity",
      "## Browser and media data",
      "## Diagnostics, logs, and exports",
      "## Retention and deletion",
    ],
  ],
  [
    "docs/THIRD_PARTY_NOTICES.md",
    [
      "## Direct runtime components",
      "## Optional/external components",
      "## Reproduce the inventory",
      "## Download Manager repository license",
    ],
  ],
]);

for (const [relativePath, headings] of requiredSections) {
  const contents = contentsByFile.get(relativePath) ?? "";
  for (const heading of headings) {
    if (!contents.includes(heading)) {
      failures.push(`${relativePath} is missing required section: ${heading}`);
    }
  }
}

function slugifyHeading(value) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .replace(/\s+/gu, "-")
    .replace(/-+/gu, "-");
}

for (const [relativePath, contents] of contentsByFile) {
  for (const match of contents.matchAll(/\[[^\]]*\]\(([^)]+)\)/gu)) {
    const destination = match[1];
    if (/^(?:https?:|mailto:)/u.test(destination)) continue;
    const [filePart, fragment] = destination.split("#", 2);
    const targetPath = path.resolve(
      path.dirname(path.join(root, relativePath)),
      decodeURIComponent(filePart || path.basename(relativePath)),
    );
    try {
      await access(targetPath);
      if (fragment && targetPath.toLowerCase().endsWith(".md")) {
        const targetContents = await readFile(targetPath, "utf8");
        const headings = [...targetContents.matchAll(/^#{1,6}\s+(.+)$/gmu)].map(
          (heading) => slugifyHeading(heading[1]),
        );
        if (!headings.includes(decodeURIComponent(fragment).toLowerCase())) {
          failures.push(
            `${relativePath} links to missing heading: ${destination}`,
          );
        }
      }
    } catch {
      failures.push(
        `${relativePath} links to missing local path: ${destination}`,
      );
    }
  }
}

const userGuide = contentsByFile.get("docs/USER_GUIDE.md") ?? "";
for (const scriptName of [
  "install.ps1",
  "native-host-registration.ps1",
  "uninstall.ps1",
]) {
  if (!userGuide.includes(scriptName)) {
    failures.push(`user guide does not document ${scriptName}`);
  }
  try {
    await access(path.join(root, "packaging", "windows", scriptName));
  } catch {
    failures.push(`documented packaging command has no script: ${scriptName}`);
  }
}

const cli = spawnSync(
  process.platform === "win32" ? "cargo.exe" : "cargo",
  ["run", "--quiet", "--locked", "-p", "download-manager-cli", "--", "--help"],
  { cwd: root, encoding: "utf8" },
);
if (cli.status !== 0) {
  failures.push(
    `documented CLI help failed (${cli.status}): ${cli.stderr.trim()}`,
  );
} else {
  for (const contract of [
    "add URL --output DIRECTORY",
    "list [--json]",
    "status JOB_ID [--json]",
    "pause|resume|cancel JOB_ID [--json]",
    "queue start NAME",
  ]) {
    if (!cli.stdout.includes(contract)) {
      failures.push(`CLI help is missing documented contract: ${contract}`);
    }
  }
}

const packageJson = JSON.parse(
  await readFile(path.join(root, "package.json"), "utf8"),
);
if (!packageJson.scripts?.["audit:licenses"]) {
  failures.push(
    "third-party inventory documents missing pnpm audit:licenses command",
  );
}
for (const scriptName of ["security-quality.ps1"]) {
  try {
    await access(path.join(root, "scripts", scriptName));
  } catch {
    failures.push(
      `third-party inventory documents missing script: ${scriptName}`,
    );
  }
}

if (!userGuide.includes("Protected or unsupported media")) {
  failures.push("user guide must state the protected-media boundary");
}
const privacy = contentsByFile.get("docs/PRIVACY.md") ?? "";
const normalizedPrivacy = privacy.replace(/\s+/gu, " ");
for (const disclosure of [
  "no analytics",
  "no automatic diagnostics upload",
  "Incognito/private capture is off by default",
  "`Keep`",
  "`Remove`",
]) {
  if (!normalizedPrivacy.includes(disclosure)) {
    failures.push(`privacy statement is missing disclosure: ${disclosure}`);
  }
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  console.log(
    `release documentation verification passed (${releaseDocs.length} documents; CLI help and packaging commands verified)`,
  );
}
