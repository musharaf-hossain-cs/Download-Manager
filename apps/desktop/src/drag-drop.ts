export type DropInput =
  | { readonly kind: "url"; readonly value: string }
  | { readonly kind: "text"; readonly value: string }
  | { readonly kind: "list"; readonly values: readonly string[] };

export interface InvalidDropItem {
  readonly inputIndex: number;
  readonly itemIndex: number;
  readonly value: string;
  readonly reason: "invalid_url" | "unsupported_scheme";
}

export interface DropNormalizationReport {
  readonly urls: readonly string[];
  readonly duplicates: readonly string[];
  readonly invalid: readonly InvalidDropItem[];
}

/** Normalizes URL, multiline text, and list drops into one bounded report. */
export function normalizeDropInputs(
  inputs: readonly DropInput[],
): DropNormalizationReport {
  const urls: string[] = [];
  const duplicates: string[] = [];
  const invalid: InvalidDropItem[] = [];
  const seen = new Set<string>();

  for (const [inputIndex, input] of inputs.entries()) {
    const values = expandInput(input);
    for (const [itemIndex, value] of values.entries()) {
      const normalized = normalizeUrl(value);
      if (normalized.url === null) {
        invalid.push({
          inputIndex,
          itemIndex,
          value: value.trim(),
          reason: normalized.reason,
        });
      } else if (seen.has(normalized.url)) {
        duplicates.push(normalized.url);
      } else {
        seen.add(normalized.url);
        urls.push(normalized.url);
      }
    }
  }

  return { urls, duplicates, invalid };
}

function expandInput(input: DropInput): readonly string[] {
  if (input.kind === "url") return [input.value];
  if (input.kind === "list") return input.values;
  return input.value
    .split(/\r?\n/u)
    .map((value) => value.trim())
    .filter((value) => value.length > 0);
}

function normalizeUrl(value: string):
  | { readonly url: string }
  | {
      readonly url: null;
      readonly reason: "invalid_url" | "unsupported_scheme";
    } {
  const candidate = value.trim();
  if (candidate.length === 0 || candidate.length > 8_192) {
    return { url: null, reason: "invalid_url" };
  }
  try {
    const url = new URL(candidate);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return { url: null, reason: "unsupported_scheme" };
    }
    if (url.hostname.length === 0) {
      return { url: null, reason: "invalid_url" };
    }
    return { url: url.toString() };
  } catch {
    return { url: null, reason: "invalid_url" };
  }
}
