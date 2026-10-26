import { describe, expect, it } from "vitest";

import {
  BANGLA_MESSAGES,
  createTextExpansionCatalog,
  ENGLISH_MESSAGES,
  LocalizationController,
  MESSAGE_KEYS,
  type MessageCatalog,
} from "../src/localization.js";

describe("externalized desktop strings", () => {
  it("selects English or Bangla from a Windows-style locale", () => {
    const english = new LocalizationController("en-US");
    const bangla = new LocalizationController("bn-BD");

    expect(english.text("actions.openFolder")).toBe("Open containing folder");
    expect(bangla.text("actions.openFolder")).toBe("ফাইলের ফোল্ডার খুলুন");
    english.setLocale("unknown");
    expect(english.locale).toBe("en");
  });

  it("rejects a runtime catalog with any missing, blank, or unknown key", () => {
    const missing = { ...BANGLA_MESSAGES } as Record<string, string>;
    delete missing["toolbar.stop"];
    expect(
      () =>
        new LocalizationController("en", {
          en: ENGLISH_MESSAGES,
          bn: missing,
        }),
    ).toThrow("bn message catalog is missing toolbar.stop");

    expect(
      () =>
        new LocalizationController("en", {
          en: { ...ENGLISH_MESSAGES, unexpected: "text" },
          bn: BANGLA_MESSAGES,
        }),
    ).toThrow("unknown key unexpected");
  });

  it("keeps both locale skeletons complete", () => {
    for (const catalog of [ENGLISH_MESSAGES, BANGLA_MESSAGES]) {
      expect(Object.keys(catalog)).toEqual([...MESSAGE_KEYS]);
      expect(
        Object.values(catalog).every((text) => text.trim().length > 0),
      ).toBe(true);
    }
  });

  it("creates a Unicode-safe 30 percent text-expansion fixture", () => {
    const expanded = createTextExpansionCatalog(ENGLISH_MESSAGES);
    for (const key of MESSAGE_KEYS) {
      expect(Array.from(expanded[key]).length).toBe(
        Math.ceil(Array.from(ENGLISH_MESSAGES[key]).length * 1.3),
      );
    }

    const banglaExpanded = createTextExpansionCatalog(
      BANGLA_MESSAGES,
      0.3,
    ) as MessageCatalog;
    expect(banglaExpanded["app.title"]).toMatch(/^ডাউনলোড ম্যানেজার/u);
    expect(() => createTextExpansionCatalog(ENGLISH_MESSAGES, -0.1)).toThrow(
      RangeError,
    );
  });
});
