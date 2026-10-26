import { describe, expect, it } from "vitest";

import {
  DEFAULT_CAPTURE_MODIFIERS,
  resolveCaptureModifier,
  type BrowserModifierState,
  type CaptureModifierConfig,
} from "../src/modifier-keys.js";

const none: BrowserModifierState = {
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
};

describe("force and bypass capture modifiers", () => {
  it("documents and applies the Windows-safe Control/Alt defaults", () => {
    expect(DEFAULT_CAPTURE_MODIFIERS).toEqual({
      force: "Control",
      bypass: "Alt",
    });
    expect(resolveCaptureModifier(none)).toEqual({
      action: "default",
      reason: "no_modifier",
    });
    expect(resolveCaptureModifier({ ...none, ctrlKey: true })).toEqual({
      action: "force",
      reason: "force_modifier",
    });
    expect(resolveCaptureModifier({ ...none, altKey: true })).toEqual({
      action: "bypass",
      reason: "bypass_modifier",
    });
  });

  it("supports configured alternatives for browser or OS shortcut conflicts", () => {
    const alternatives: CaptureModifierConfig = {
      force: "Control",
      bypass: "Meta",
    };
    expect(
      resolveCaptureModifier({ ...none, ctrlKey: true }, alternatives),
    ).toEqual({ action: "force", reason: "force_modifier" });
    expect(
      resolveCaptureModifier({ ...none, metaKey: true }, alternatives),
    ).toEqual({ action: "bypass", reason: "bypass_modifier" });
  });

  it("uses the safer bypass action when both configured modifiers are held", () => {
    expect(
      resolveCaptureModifier({ ...none, altKey: true, ctrlKey: true }),
    ).toEqual({ action: "bypass", reason: "modifier_conflict" });
  });

  it("rejects identical or unsupported configured modifiers", () => {
    expect(() =>
      resolveCaptureModifier(none, { force: "Alt", bypass: "Alt" }),
    ).toThrow(/must be different/);
    expect(() =>
      resolveCaptureModifier(none, {
        force: "CapsLock",
        bypass: "Shift",
      } as unknown as CaptureModifierConfig),
    ).toThrow(/supported keyboard modifiers/);
  });
});
