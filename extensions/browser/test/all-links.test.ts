import { describe, expect, it } from "vitest";

import {
  collectAllLinks,
  type AllLinksDocumentSource,
  type LinkElementSource,
} from "../src/all-links.js";

function documentFixture(
  hrefs: Iterable<string | null>,
  baseURI = "https://www.example.test/releases/current/index.html#section",
): AllLinksDocumentSource {
  return {
    baseURI,
    querySelectorAll(selector) {
      expect(selector).toBe("a[href], area[href]");
      return {
        *[Symbol.iterator](): Iterator<LinkElementSource> {
          for (const href of hrefs) {
            yield { getAttribute: () => href };
          }
        },
      };
    },
  };
}

describe("all-links collection", () => {
  it("normalizes direct and relative HTTP(S) links before stable deduplication", async () => {
    const collection = await collectAllLinks(
      documentFixture([
        "../app.zip#mirror-one",
        "https://www.example.test:443/releases/app.zip#mirror-two",
        "//cdn.example.test/files/tool.exe",
        "mailto:help@example.test",
        null,
      ]),
    );

    expect(collection).toEqual({
      items: [
        {
          url: "https://www.example.test/releases/app.zip",
          referrer: "https://www.example.test/releases/current/index.html",
          suggested_filename: null,
        },
        {
          url: "https://cdn.example.test/files/tool.exe",
          referrer: "https://www.example.test/releases/current/index.html",
          suggested_filename: null,
        },
      ],
      scannedCount: 5,
      duplicateCount: 1,
      rejectedCount: 2,
      limitReached: null,
    });
  });

  it("stops exactly at the native batch cap without consuming another link", async () => {
    let generatedCount = 0;
    const hrefs: Iterable<string> = {
      *[Symbol.iterator]() {
        for (let index = 0; index < 200; index += 1) {
          generatedCount += 1;
          yield `/downloads/file-${String(index)}.zip`;
        }
      },
    };
    const collection = await collectAllLinks(documentFixture(hrefs), {
      maxUniqueLinks: 128,
    });

    expect(collection.items).toHaveLength(128);
    expect(collection.scannedCount).toBe(128);
    expect(generatedCount).toBe(128);
    expect(collection.limitReached).toBe("unique");
    expect(new Set(collection.items.map(({ url }) => url)).size).toBe(128);
  });

  it("cooperatively yields during a large bounded DOM-like scan", async () => {
    let generatedCount = 0;
    const hrefs: Iterable<string> = {
      *[Symbol.iterator]() {
        for (let index = 0; index < 30_000; index += 1) {
          generatedCount += 1;
          yield `/large/file-${String(index % 64)}.zip#${String(index)}`;
        }
      },
    };
    const yieldAt: number[] = [];
    const collection = await collectAllLinks(documentFixture(hrefs), {
      maxScannedLinks: 20_000,
      batchSize: 250,
      scheduler: {
        async yield() {
          yieldAt.push(generatedCount);
          await Promise.resolve();
        },
      },
    });

    expect(collection).toMatchObject({
      scannedCount: 20_000,
      duplicateCount: 19_936,
      rejectedCount: 0,
      limitReached: "scanned",
    });
    expect(collection.items).toHaveLength(64);
    expect(yieldAt).toHaveLength(80);
    expect(yieldAt).toEqual(
      Array.from({ length: 80 }, (_, index) => (index + 1) * 250),
    );
    expect(yieldAt[0]).toBeLessThan(collection.scannedCount);
    expect(generatedCount).toBe(20_000);
  });

  it("rejects invalid page origins and unbounded option values", async () => {
    await expect(
      collectAllLinks(documentFixture([], "file:///C:/private/index.html")),
    ).rejects.toThrow("absolute HTTP(S)");
    await expect(
      collectAllLinks(documentFixture([]), { maxUniqueLinks: 129 }),
    ).rejects.toThrow("maxUniqueLinks");
    await expect(
      collectAllLinks(documentFixture([]), { maxScannedLinks: 100_001 }),
    ).rejects.toThrow("maxScannedLinks");
  });
});
