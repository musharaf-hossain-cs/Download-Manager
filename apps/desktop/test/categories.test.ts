import { describe, expect, it } from "vitest";

import {
  createDefaultCategories,
  matchCategoryRules,
  resolveCategoryDestination,
  type CategoryMatchInput,
  type CategoryRule,
} from "../src/categories.js";

describe("default categories", () => {
  it("creates stable defaults and category-specific destinations", () => {
    const first = createDefaultCategories(" C:\\Users\\Ada\\Downloads\\ ");
    const second = createDefaultCategories("C:\\Users\\Ada\\Downloads");

    expect(first).toEqual(second);
    expect(first).toEqual([
      category("general", "General", "C:\\Users\\Ada\\Downloads"),
      category(
        "compressed",
        "Compressed",
        "C:\\Users\\Ada\\Downloads\\Compressed",
      ),
      category(
        "documents",
        "Documents",
        "C:\\Users\\Ada\\Downloads\\Documents",
      ),
      category("music", "Music", "C:\\Users\\Ada\\Downloads\\Music"),
      category("programs", "Programs", "C:\\Users\\Ada\\Downloads\\Programs"),
      category("video", "Video", "C:\\Users\\Ada\\Downloads\\Video"),
    ]);
  });

  it("uses a remembered folder only when its policy enables it", () => {
    const [general] = createDefaultCategories("D:\\Downloads");
    if (general === undefined) throw new Error("missing General category");

    expect(
      resolveCategoryDestination({
        ...general,
        destination: {
          ...general.destination,
          lastFolder: " E:\\Recent\\ ",
        },
      }),
    ).toBe("E:\\Recent");
    expect(
      resolveCategoryDestination({
        ...general,
        destination: {
          ...general.destination,
          rememberLastFolder: false,
          lastFolder: "E:\\Recent",
        },
      }),
    ).toBe("D:\\Downloads");
  });

  it("rejects an empty downloads folder", () => {
    expect(() => createDefaultCategories("  ")).toThrow(
      "downloads folder must not be empty",
    );
  });
});

describe("ordered category rules", () => {
  const rules: readonly CategoryRule[] = [
    {
      id: "host-release",
      categoryId: "programs",
      kind: "host",
      value: "downloads.example.com",
    },
    {
      id: "zip-file",
      categoryId: "compressed",
      kind: "extension",
      value: ".ZIP",
    },
    {
      id: "video-content",
      categoryId: "video",
      kind: "mime",
      value: "video/*",
    },
  ];

  it.each([
    {
      label: "host precedes extension and MIME",
      input: candidate("zip", "video/mp4", "DOWNLOADS.EXAMPLE.COM."),
      expected: {
        categoryId: "programs",
        matchedRuleId: "host-release",
        priority: 1,
      },
    },
    {
      label: "extension precedes MIME after the host misses",
      input: candidate(".Zip", "video/mp4", "cdn.example.com"),
      expected: {
        categoryId: "compressed",
        matchedRuleId: "zip-file",
        priority: 2,
      },
    },
    {
      label: "MIME wildcard handles parameters and case",
      input: candidate("bin", "Video/MP4; charset=binary", "cdn.example.com"),
      expected: {
        categoryId: "video",
        matchedRuleId: "video-content",
        priority: 3,
      },
    },
  ])("uses first-match precedence when $label", ({ input, expected }) => {
    expect(matchCategoryRules(input, rules)).toEqual(expected);
  });

  it("falls back to General when no ordered rule matches", () => {
    expect(
      matchCategoryRules(
        candidate("txt", "text/plain", "documents.example.com"),
        rules,
      ),
    ).toEqual({ categoryId: "general", matchedRuleId: null, priority: null });
  });
});

function category(id: string, name: string, defaultFolder: string) {
  return {
    id,
    name,
    destination: {
      defaultFolder,
      rememberLastFolder: true,
      lastFolder: null,
    },
  };
}

function candidate(
  extension: string | null,
  mimeType: string | null,
  sourceHost: string | null,
): CategoryMatchInput {
  return { extension, mimeType, sourceHost };
}
