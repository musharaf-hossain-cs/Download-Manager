import { createHash } from "node:crypto";
import { Buffer } from "node:buffer";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, resolve } from "node:path";

const options = parseArguments(process.argv.slice(2));
const host = "127.0.0.1";

const fixtures = new Map([
  [
    "/small.txt",
    {
      filename: "pu09-small.txt",
      body: Buffer.from("PU09 installed acceptance fixture\r\n", "utf8"),
      delayMs: 0,
      chunked: false,
      mutableIdentity: false,
    },
  ],
  [
    "/edge.txt",
    {
      filename: "pu12-edge.txt",
      body: Buffer.from("PU12 installed Edge capture fixture\r\n", "utf8"),
      delayMs: 0,
      chunked: false,
      mutableIdentity: false,
    },
  ],
  [
    "/chrome-capture.bin",
    {
      filename: "pu12-chrome-capture.bin",
      size: 4 * 1024 * 1024 + 17,
      delayMs: 20,
      chunked: false,
      mutableIdentity: false,
    },
  ],
  [
    "/edge-capture.bin",
    {
      filename: "pu12-edge-capture.bin",
      size: 4 * 1024 * 1024 + 29,
      delayMs: 20,
      chunked: false,
      mutableIdentity: false,
    },
  ],
  [
    "/fallback.txt",
    {
      filename: "pu12-browser-fallback.txt",
      body: Buffer.from("PU12 offline browser fallback fixture\r\n", "utf8"),
      delayMs: 0,
      chunked: false,
      mutableIdentity: false,
    },
  ],
  [
    "/large.bin",
    {
      filename: "pu09-large.bin",
      size: 64 * 1024 * 1024 + 137,
      delayMs: 25,
      chunked: false,
      mutableIdentity: false,
    },
  ],
  [
    "/unknown.bin",
    {
      filename: "pu09-unknown.bin",
      size: 1024 * 1024 + 19,
      delayMs: 4,
      chunked: true,
      mutableIdentity: false,
    },
  ],
  [
    "/cancel.bin",
    {
      filename: "pu09-cancel.bin",
      size: 32 * 1024 * 1024 + 73,
      delayMs: 25,
      chunked: false,
      mutableIdentity: false,
    },
  ],
  [
    "/changing.bin",
    {
      filename: "pu09-changing.bin",
      size: 32 * 1024 * 1024 + 211,
      delayMs: 25,
      chunked: false,
      mutableIdentity: true,
    },
  ],
]);

for (const fixture of fixtures.values()) {
  fixture.size ??= fixture.body.length;
  fixture.sha256 = hashFixture(fixture);
}

await mkdir(dirname(options.identityFile), { recursive: true });
try {
  await readFile(options.identityFile, "utf8");
} catch (error) {
  if (error?.code !== "ENOENT") throw error;
  await writeFile(options.identityFile, "v1\n", "utf8");
}

const server = createServer(async (request, response) => {
  try {
    const url = new globalThis.URL(
      request.url ?? "/",
      `http://${host}:${options.port}`,
    );
    if (url.pathname === "/browser.html" && request.method === "GET") {
      const body = Buffer.from(
        '<!doctype html><title>PU12 browser acceptance</title><h1>PU12 installed browser capture</h1><a id="chrome-capture" href="/chrome-capture.bin">Chrome capture fixture</a><a id="edge-capture" href="/edge-capture.bin">Edge capture fixture</a><a id="offline-fallback" href="/fallback.txt">Offline fallback fixture</a>',
        "utf8",
      );
      response.writeHead(200, {
        "Content-Type": "text/html; charset=utf-8",
        "Content-Length": String(body.length),
        Connection: "close",
      });
      response.end(body);
      return;
    }
    const fixture = fixtures.get(url.pathname);
    if (
      fixture === undefined ||
      !["GET", "HEAD"].includes(request.method ?? "")
    ) {
      response.writeHead(404, {
        "Content-Type": "text/plain",
        Connection: "close",
      });
      response.end("not found\n");
      return;
    }
    await serveFixture(request, response, fixture);
  } catch (error) {
    if (!response.headersSent) {
      response.writeHead(500, {
        "Content-Type": "text/plain",
        Connection: "close",
      });
    }
    response.end(
      `fixture error: ${error instanceof Error ? error.message : String(error)}\n`,
    );
  }
});

server.on("clientError", (_error, socket) => socket.destroy());
server.listen(options.port, host, async () => {
  const baseUrl = `http://${host}:${options.port}`;
  const manifest = {
    schema_version: 1,
    host,
    port: options.port,
    browser_page_url: `${baseUrl}/browser.html`,
    identity_file: options.identityFile,
    fixtures: Object.fromEntries(
      [...fixtures.entries()].map(([path, fixture]) => [
        path.slice(1, -4),
        {
          url: `${baseUrl}${path}`,
          filename: fixture.filename,
          size: fixture.size,
          sha256: fixture.sha256,
          content_length_reported: !fixture.chunked,
          range_supported: !fixture.chunked,
          mutable_identity: fixture.mutableIdentity,
        },
      ]),
    ),
  };
  await mkdir(dirname(options.manifest), { recursive: true });
  await writeFile(
    options.manifest,
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );
  process.stdout.write(
    `${JSON.stringify({ ready: true, manifest: options.manifest })}\n`,
  );
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}

async function serveFixture(request, response, fixture) {
  const identity = fixture.mutableIdentity
    ? (await readFile(options.identityFile, "utf8")).trim()
    : "stable-v1";
  if (!/^[A-Za-z0-9._-]{1,32}$/u.test(identity)) {
    throw new Error("identity file contains an invalid value");
  }
  const etag = `"${fixture.filename}-${identity}"`;
  const headers = {
    "Content-Type": fixture.filename.endsWith(".txt")
      ? "text/plain; charset=utf-8"
      : "application/octet-stream",
    "Content-Disposition": `attachment; filename="${fixture.filename}"`,
    ETag: etag,
    Connection: "close",
  };

  if (fixture.chunked) {
    response.writeHead(200, headers);
    if (request.method === "HEAD") {
      response.end();
      return;
    }
    await sendBody(response, fixture, 0, fixture.size - 1);
    return;
  }

  headers["Accept-Ranges"] = "bytes";
  const range = parseRange(request.headers.range, fixture.size);
  if (range === null) {
    headers["Content-Length"] = String(fixture.size);
    response.writeHead(200, headers);
    if (request.method === "HEAD") {
      response.end();
      return;
    }
    await sendBody(response, fixture, 0, fixture.size - 1);
    return;
  }

  headers["Content-Length"] = String(range.end - range.start + 1);
  headers["Content-Range"] =
    `bytes ${range.start}-${range.end}/${fixture.size}`;
  response.writeHead(206, headers);
  if (request.method === "HEAD") {
    response.end();
    return;
  }
  await sendBody(response, fixture, range.start, range.end);
}

async function sendBody(response, fixture, start, end) {
  const chunkSize = 64 * 1024;
  for (let offset = start; offset <= end; offset += chunkSize) {
    if (response.destroyed) return;
    const length = Math.min(chunkSize, end - offset + 1);
    const chunk =
      fixture.body?.subarray(offset, offset + length) ??
      patterned(offset, length);
    if (!response.write(chunk)) {
      await new Promise((resolveDrain) => response.once("drain", resolveDrain));
    }
    if (fixture.delayMs > 0) await delay(fixture.delayMs);
  }
  response.end();
}

function patterned(start, length) {
  const body = Buffer.allocUnsafe(length);
  for (let index = 0; index < length; index += 1) {
    body[index] = ((start + index) * 31 + 17) % 251;
  }
  return body;
}

function hashFixture(fixture) {
  const hash = createHash("sha256");
  const chunkSize = 1024 * 1024;
  for (let offset = 0; offset < fixture.size; offset += chunkSize) {
    const length = Math.min(chunkSize, fixture.size - offset);
    hash.update(
      fixture.body?.subarray(offset, offset + length) ??
        patterned(offset, length),
    );
  }
  return hash.digest("hex");
}

function parseRange(value, size) {
  if (value === undefined) return null;
  const match = /^bytes=(\d+)-(\d*)$/u.exec(value);
  if (match === null)
    throw new Error("only one bounded byte range is supported");
  const start = Number(match[1]);
  const end = match[2] === "" ? size - 1 : Number(match[2]);
  if (
    !Number.isSafeInteger(start) ||
    !Number.isSafeInteger(end) ||
    start > end ||
    end >= size
  ) {
    throw new Error("invalid byte range");
  }
  return { start, end };
}

function parseArguments(arguments_) {
  let port = 49321;
  let manifest = resolve("test-results/pu09/fixture-manifest.json");
  let identityFile = resolve("test-results/pu09/changing-identity.txt");
  for (let index = 0; index < arguments_.length; index += 1) {
    const name = arguments_[index];
    const value = arguments_[index + 1];
    if (name === "--port" && value !== undefined) {
      port = Number(value);
      index += 1;
    } else if (name === "--manifest" && value !== undefined) {
      manifest = resolve(value);
      index += 1;
    } else if (name === "--identity-file" && value !== undefined) {
      identityFile = resolve(value);
      index += 1;
    } else {
      throw new Error(`unknown or incomplete argument: ${name}`);
    }
  }
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    throw new Error("--port must be an integer from 1024 through 65535");
  }
  return { port, manifest, identityFile };
}

function delay(milliseconds) {
  return new Promise((resolveDelay) =>
    globalThis.setTimeout(resolveDelay, milliseconds),
  );
}
