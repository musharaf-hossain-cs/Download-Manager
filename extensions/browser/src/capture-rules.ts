export interface CaptureRuleConfig {
  readonly extensions: readonly string[];
  readonly mimeTypes: readonly string[];
  readonly minimumSizeBytes: number;
}

export interface CaptureRuleCandidate {
  readonly url: string;
  readonly suggestedFilename: string | null;
  readonly mimeType: string | null;
  readonly sizeBytes: number | null;
}

export type CaptureRuleDecision =
  | {
      readonly capture: true;
      readonly reason: "extension_match" | "mime_match";
      readonly matchedRule: string;
    }
  | {
      readonly capture: false;
      readonly reason:
        "below_minimum_size" | "invalid_candidate" | "no_type_match";
    };

/** Applies minimum-size, extension, and MIME rules in deterministic order. */
export function evaluateCaptureRules(
  candidate: CaptureRuleCandidate,
  config: CaptureRuleConfig,
): CaptureRuleDecision {
  validateConfig(config);
  if (
    candidate.sizeBytes !== null &&
    (!Number.isSafeInteger(candidate.sizeBytes) || candidate.sizeBytes < 0)
  ) {
    return { capture: false, reason: "invalid_candidate" };
  }
  if (
    candidate.sizeBytes !== null &&
    candidate.sizeBytes < config.minimumSizeBytes
  ) {
    return { capture: false, reason: "below_minimum_size" };
  }

  const extension = candidateExtension(candidate);
  for (const configured of config.extensions) {
    const rule = normalizeExtension(configured);
    if (rule === extension) {
      return { capture: true, reason: "extension_match", matchedRule: rule };
    }
  }

  const mimeType = normalizeCandidateMime(candidate.mimeType);
  if (mimeType !== null) {
    for (const configured of config.mimeTypes) {
      const rule = normalizeMimeRule(configured);
      if (matchesMime(mimeType, rule)) {
        return { capture: true, reason: "mime_match", matchedRule: rule };
      }
    }
  }
  return { capture: false, reason: "no_type_match" };
}

function validateConfig(config: CaptureRuleConfig): void {
  if (
    !Number.isSafeInteger(config.minimumSizeBytes) ||
    config.minimumSizeBytes < 0
  ) {
    throw new RangeError("minimum size must be a non-negative safe integer");
  }
  for (const extension of config.extensions) normalizeExtension(extension);
  for (const mimeType of config.mimeTypes) normalizeMimeRule(mimeType);
}

function candidateExtension(candidate: CaptureRuleCandidate): string | null {
  const filename =
    candidate.suggestedFilename ?? filenameFromUrl(candidate.url);
  if (filename === null) return null;
  const lastDot = filename.lastIndexOf(".");
  if (lastDot < 0 || lastDot === filename.length - 1) return null;
  return filename.slice(lastDot + 1).toLowerCase();
}

function filenameFromUrl(value: string): string | null {
  try {
    const url = new URL(value);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    const filename = url.pathname.split("/").at(-1);
    return filename === undefined ? null : decodeURIComponent(filename);
  } catch {
    return null;
  }
}

function normalizeExtension(value: string): string {
  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/^\*?\./u, "");
  if (!/^[a-z0-9][a-z0-9._+-]{0,31}$/u.test(normalized)) {
    throw new Error(`invalid extension rule: ${value}`);
  }
  return normalized;
}

function normalizeCandidateMime(value: string | null): string | null {
  if (value === null) return null;
  const normalized = value.split(";", 1)[0]?.trim().toLowerCase();
  return normalized !== undefined && /^[^\s/]+\/[^\s/]+$/u.test(normalized)
    ? normalized
    : null;
}

function normalizeMimeRule(value: string): string {
  const normalized = value.trim().toLowerCase();
  if (!/^[^\s/*]+\/(?:[^\s/*]+|\*)$/u.test(normalized)) {
    throw new Error(`invalid MIME rule: ${value}`);
  }
  return normalized;
}

function matchesMime(mimeType: string, rule: string): boolean {
  return rule.endsWith("/*")
    ? mimeType.startsWith(rule.slice(0, -1))
    : mimeType === rule;
}
