import { describe, expect, it } from "vitest";

import {
  createDefaultCategories,
  resolveCategoryDestination,
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
