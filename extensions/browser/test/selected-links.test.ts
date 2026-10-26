import { describe, expect, it } from "vitest";

import {
  collectSelectedLinks,
  type LinkElementSource,
  type SelectedLinksDocumentSource,
  type SelectedLinksRangeSource,
} from "../src/index.js";

const BASE_URL = "https://www.example.test/releases/current/index.html#notes";

function rangeFixture(
  hrefs: readonly (string | null)[],
): SelectedLinksRangeSource {
  return {
    cloneContents() {
      return {
        querySelectorAll(selector) {
          expect(selector).toBe("a[href], area[href]");
          return hrefs.map<LinkElementSource>((href) => ({
            getAttribute: () => href,
          }));
        },
      };
    },
  };
}

function documentFixture(
  ranges: readonly SelectedLinksRangeSource[] | null,
): SelectedLinksDocumentSource {
  return {
    baseURI: BASE_URL,
    getSelection() {
      if (ranges === null) return null;
      return {
        rangeCount: ranges.length,
        getRangeAt: (index) => {
          const range = ranges[index];
          if (range === undefined) throw new RangeError("missing range");
          return range;
        },
      };
    },
  };
}

describe("selected-links collection", () => {
  it("collects only links cloned from the current selection", async () => {
    const collection = await collectSelectedLinks(
      documentFixture([
        rangeFixture([
          "../selected.zip#mirror",
          "//cdn.example.test/selected.exe",
          "mailto:help@example.test",
        ]),
      ]),
    );

    expect(collection).toEqual({
      items: [
        {
          url: "https://www.example.test/releases/selected.zip",
          referrer: "https://www.example.test/releases/current/index.html",
          suggested_filename: null,
        },
        {
          url: "https://cdn.example.test/selected.exe",
          referrer: "https://www.example.test/releases/current/index.html",
          suggested_filename: null,
        },
      ],
      scannedCount: 3,
      duplicateCount: 0,
      rejectedCount: 1,
      limitReached: null,
    });
  });

  it("deduplicates normalized links across overlapping selection ranges", async () => {
    const collection = await collectSelectedLinks(
      documentFixture([
        rangeFixture(["../app.zip#first", "https://cdn.example.test/tool.exe"]),
        rangeFixture([
          "https://www.example.test:443/releases/app.zip#second",
          "https://cdn.example.test/tool.exe#fragment",
        ]),
      ]),
    );

    expect(collection.items.map(({ url }) => url)).toEqual([
      "https://www.example.test/releases/app.zip",
      "https://cdn.example.test/tool.exe",
    ]);
    expect(collection).toMatchObject({
      scannedCount: 4,
      duplicateCount: 2,
      rejectedCount: 0,
    });
  });

  it("returns an empty batch when the page has no active selection", async () => {
    await expect(collectSelectedLinks(documentFixture(null))).resolves.toEqual({
      items: [],
      scannedCount: 0,
      duplicateCount: 0,
      rejectedCount: 0,
      limitReached: null,
    });
  });
});
