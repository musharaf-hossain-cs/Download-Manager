import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const temporaryRoot = await mkdtemp(
  join(tmpdir(), "download-manager-pu09-fixture-"),
);
const manifestPath = join(temporaryRoot, "manifest.json");
const identityPath = join(temporaryRoot, "identity.txt");
const port = 49322;
const child = spawn(
  process.execPath,
  [
    "scripts/private-use-acceptance-fixture.mjs",
    "--port",
    String(port),
    "--manifest",
    manifestPath,
    "--identity-file",
    identityPath,
  ],
  { stdio: ["ignore", "pipe", "pipe"] },
);

try {
  await waitForReady(child);
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  assert.equal(manifest.schema_version, 1);
  assert.equal(manifest.port, port);

  const small = await globalThis.fetch(manifest.fixtures.small.url);
  const smallBytes = Buffer.from(await small.arrayBuffer());
  assert.equal(small.status, 200);
  assert.equal(small.headers.get("content-length"), String(smallBytes.length));
  assert.equal(sha256(smallBytes), manifest.fixtures.small.sha256);

  const unknown = await globalThis.fetch(manifest.fixtures.unknown.url);
  const unknownBytes = Buffer.from(await unknown.arrayBuffer());
  assert.equal(unknown.status, 200);
  assert.equal(unknown.headers.get("content-length"), null);
  assert.equal(sha256(unknownBytes), manifest.fixtures.unknown.sha256);

  const range = await globalThis.fetch(manifest.fixtures.large.url, {
    headers: { Range: "bytes=1024-2047" },
  });
  assert.equal(range.status, 206);
  assert.equal(
    range.headers.get("content-range"),
    `bytes 1024-2047/${manifest.fixtures.large.size}`,
  );
  assert.equal((await range.arrayBuffer()).byteLength, 1024);

  const firstIdentity = await globalThis.fetch(manifest.fixtures.changing.url, {
    method: "HEAD",
  });
  await writeFile(identityPath, "v2\n", "utf8");
  const secondIdentity = await globalThis.fetch(
    manifest.fixtures.changing.url,
    {
      method: "HEAD",
    },
  );
  assert.notEqual(
    firstIdentity.headers.get("etag"),
    secondIdentity.headers.get("etag"),
  );
} finally {
  child.kill("SIGTERM");
  await new Promise((resolve) => child.once("exit", resolve));
  await rm(temporaryRoot, { recursive: true, force: true });
}

console.log("PU09 deterministic fixture server checks passed.");

function waitForReady(process_) {
  return new Promise((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    const timeout = globalThis.setTimeout(() => {
      reject(new Error(`fixture server did not become ready: ${stderr}`));
    }, 10_000);
    process_.stderr.setEncoding("utf8");
    process_.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    process_.stdout.setEncoding("utf8");
    process_.stdout.on("data", (chunk) => {
      stdout += chunk;
      if (stdout.includes('"ready":true')) {
        globalThis.clearTimeout(timeout);
        resolve();
      }
    });
    process_.once("exit", (code) => {
      globalThis.clearTimeout(timeout);
      reject(
        new Error(`fixture server exited early with code ${code}: ${stderr}`),
      );
    });
  });
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
