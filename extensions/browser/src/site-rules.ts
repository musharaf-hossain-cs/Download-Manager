export type CapturePolicy = "allow" | "deny";

export interface SiteCaptureRule {
  readonly hostPattern: string;
  readonly policy: CapturePolicy;
}

export interface SiteCaptureConfig {
  readonly globalPolicy: CapturePolicy;
  readonly sites: readonly SiteCaptureRule[];
}

export type SiteCaptureDecision =
  | {
      readonly capture: boolean;
      readonly reason: "site_allow" | "site_deny";
      readonly matchedPattern: string;
    }
  | {
      readonly capture: boolean;
      readonly reason: "global_allow" | "global_deny";
    }
  | { readonly capture: false; readonly reason: "invalid_site_url" };

/** Resolves exact and wildcard site rules before the global capture policy. */
export function evaluateSiteCapture(
  siteUrl: string,
  config: SiteCaptureConfig,
): SiteCaptureDecision {
  const host = siteHost(siteUrl);
  if (host === null) return { capture: false, reason: "invalid_site_url" };

  let winner:
    | {
        readonly rule: SiteCaptureRule;
        readonly pattern: string;
        score: number;
      }
    | undefined;
  for (const rule of config.sites) {
    const pattern = normalizeHostPattern(rule.hostPattern);
    const score = matchScore(host, pattern);
    if (
      score !== null &&
      (winner === undefined ||
        score > winner.score ||
        (score === winner.score &&
          rule.policy === "deny" &&
          winner.rule.policy === "allow"))
    ) {
      winner = { rule, pattern, score };
    }
  }
  if (winner !== undefined) {
    const capture = winner.rule.policy === "allow";
    return {
      capture,
      reason: capture ? "site_allow" : "site_deny",
      matchedPattern: winner.pattern,
    };
  }

  const capture = config.globalPolicy === "allow";
  return { capture, reason: capture ? "global_allow" : "global_deny" };
}

function siteHost(value: string): string | null {
  try {
    const url = new URL(value);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    return url.hostname.toLowerCase().replace(/\.$/u, "");
  } catch {
    return null;
  }
}

function normalizeHostPattern(value: string): string {
  const pattern = value.trim().toLowerCase().replace(/\.$/u, "");
  if (pattern === "*") return pattern;
  const host = pattern.startsWith("*.") ? pattern.slice(2) : pattern;
  if (
    host.length === 0 ||
    host.length > 253 ||
    !/^[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$/u.test(host) ||
    host.includes("..")
  ) {
    throw new Error(`invalid site host pattern: ${value}`);
  }
  return pattern.startsWith("*.") ? `*.${host}` : host;
}

function matchScore(host: string, pattern: string): number | null {
  if (pattern === "*") return 0;
  if (!pattern.startsWith("*.")) {
    return host === pattern ? 10_000 + pattern.length : null;
  }
  const suffix = pattern.slice(1);
  return host.endsWith(suffix) && host.length > suffix.length
    ? pattern.length
    : null;
}
