export type ClipboardDecision =
  | { readonly kind: "prompt"; readonly url: string }
  | {
      readonly kind: "ignored";
      readonly reason: "disabled" | "invalid" | "duplicate";
    };

/** Optional clipboard URL detection with deterministic duplicate suppression. */
export class ClipboardUrlDetector {
  #enabled: boolean;
  #lastPrompt: { readonly url: string; readonly at: number } | null = null;

  public constructor(
    options: { readonly enabled?: boolean; readonly debounceMs?: number } = {},
  ) {
    this.#enabled = options.enabled ?? false;
    this.debounceMs = options.debounceMs ?? 2_000;
    if (!Number.isFinite(this.debounceMs) || this.debounceMs < 0) {
      throw new RangeError("clipboard debounce must be a non-negative number");
    }
  }

  public readonly debounceMs: number;

  public setEnabled(enabled: boolean): void {
    this.#enabled = enabled;
  }

  public inspect(text: string, at: number): ClipboardDecision {
    if (!this.#enabled) return { kind: "ignored", reason: "disabled" };
    const url = normalizeClipboardUrl(text);
    if (url === null) return { kind: "ignored", reason: "invalid" };
    if (
      this.#lastPrompt?.url === url &&
      at >= this.#lastPrompt.at &&
      at - this.#lastPrompt.at < this.debounceMs
    ) {
      return { kind: "ignored", reason: "duplicate" };
    }
    this.#lastPrompt = { url, at };
    return { kind: "prompt", url };
  }
}

function normalizeClipboardUrl(text: string): string | null {
  const candidate = text.trim();
  if (candidate.length === 0 || candidate.length > 8_192) return null;
  try {
    const url = new URL(candidate);
    return (url.protocol === "http:" || url.protocol === "https:") &&
      url.hostname.length > 0
      ? url.toString()
      : null;
  } catch {
    return null;
  }
}
