export type CaptureModifierKey = "Alt" | "Control" | "Meta" | "Shift";

export interface CaptureModifierConfig {
  readonly force: CaptureModifierKey;
  readonly bypass: CaptureModifierKey;
}

export interface BrowserModifierState {
  readonly altKey: boolean;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly shiftKey: boolean;
}

export type CaptureModifierDecision =
  | { readonly action: "default"; readonly reason: "no_modifier" }
  | { readonly action: "force"; readonly reason: "force_modifier" }
  | { readonly action: "bypass"; readonly reason: "bypass_modifier" }
  | {
      readonly action: "bypass";
      readonly reason: "modifier_conflict";
    };

/**
 * Windows defaults avoid the OS-reserved Meta key. Either key can be changed to
 * another supported modifier when a browser, OS, or accessibility shortcut
 * conflicts with the default.
 */
export const DEFAULT_CAPTURE_MODIFIERS: CaptureModifierConfig = Object.freeze({
  force: "Control",
  bypass: "Alt",
});

const STATE_KEY: Readonly<
  Record<CaptureModifierKey, keyof BrowserModifierState>
> = Object.freeze({
  Alt: "altKey",
  Control: "ctrlKey",
  Meta: "metaKey",
  Shift: "shiftKey",
});

/** Resolves modifier intent; bypass wins when both configured keys are held. */
export function resolveCaptureModifier(
  state: BrowserModifierState,
  config: CaptureModifierConfig = DEFAULT_CAPTURE_MODIFIERS,
): CaptureModifierDecision {
  validateConfig(config);
  const force = state[STATE_KEY[config.force]];
  const bypass = state[STATE_KEY[config.bypass]];

  if (force && bypass) {
    return { action: "bypass", reason: "modifier_conflict" };
  }
  if (bypass) return { action: "bypass", reason: "bypass_modifier" };
  if (force) return { action: "force", reason: "force_modifier" };
  return { action: "default", reason: "no_modifier" };
}

function validateConfig(config: CaptureModifierConfig): void {
  if (!(config.force in STATE_KEY) || !(config.bypass in STATE_KEY)) {
    throw new RangeError(
      "capture modifiers must be supported keyboard modifiers",
    );
  }
  if (config.force === config.bypass) {
    throw new RangeError("force and bypass modifiers must be different");
  }
}
