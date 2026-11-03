export const YOUTUBE_DOWNLOAD_SCOPE_NOTICE =
  "YouTube downloads are not universally available. The download action is offered only when applicable service permission, media rights, and lawful use are all confirmed.";

export type YouTubePolicyConfirmation =
  "confirmed" | "not_confirmed" | "prohibited";

export interface YouTubeMediaAuthorization {
  readonly servicePermission: YouTubePolicyConfirmation;
  readonly mediaRights: YouTubePolicyConfirmation;
  readonly lawfulUse: YouTubePolicyConfirmation;
}

export type YouTubePolicyWithheldReason =
  | "service_prohibits_download"
  | "service_permission_required"
  | "media_rights_missing"
  | "media_rights_confirmation_required"
  | "lawful_use_not_permitted"
  | "lawful_use_confirmation_required";

export type MediaDownloadOfferDecision =
  | {
      readonly offer: true;
      readonly site: "other" | "youtube";
      readonly notice: string | null;
    }
  | {
      readonly offer: false;
      readonly site: "youtube";
      readonly reason: YouTubePolicyWithheldReason;
      readonly explanation: string;
      readonly notice: typeof YOUTUBE_DOWNLOAD_SCOPE_NOTICE;
    };

const WITHHELD_EXPLANATIONS: Readonly<
  Record<YouTubePolicyWithheldReason, string>
> = Object.freeze({
  service_prohibits_download:
    "Download unavailable: the applicable YouTube service permission or terms prohibit this download.",
  service_permission_required:
    "Download action withheld: confirm that the applicable YouTube service permission or terms allow this download.",
  media_rights_missing:
    "Download unavailable: you indicated that you do not own this media or have authorization from its rights holder.",
  media_rights_confirmation_required:
    "Download action withheld: confirm that you own this media or have authorization from its rights holder.",
  lawful_use_not_permitted:
    "Download unavailable: you indicated that this download is not permitted by applicable law.",
  lawful_use_confirmation_required:
    "Download action withheld: confirm that this download is permitted by applicable law.",
});

/**
 * Fail-closed offer policy for YouTube pages. This is an eligibility gate, not
 * legal advice or a promise that a technically detectable item can be fetched.
 */
export function evaluateMediaDownloadOffer(
  pageUrl: string,
  authorization?: YouTubeMediaAuthorization | null,
): MediaDownloadOfferDecision {
  const url = parsePageUrl(pageUrl);
  if (!isYouTubeHost(url.hostname)) {
    return Object.freeze({ offer: true, site: "other", notice: null });
  }

  const evidence = authorization ?? {
    servicePermission: "not_confirmed",
    mediaRights: "not_confirmed",
    lawfulUse: "not_confirmed",
  };
  const reason = firstWithheldReason(evidence);
  if (reason !== null) {
    return Object.freeze({
      offer: false,
      site: "youtube",
      reason,
      explanation: WITHHELD_EXPLANATIONS[reason],
      notice: YOUTUBE_DOWNLOAD_SCOPE_NOTICE,
    });
  }
  return Object.freeze({
    offer: true,
    site: "youtube",
    notice: YOUTUBE_DOWNLOAD_SCOPE_NOTICE,
  });
}

function parsePageUrl(value: string): URL {
  const url = new URL(value);
  if (
    (url.protocol !== "http:" && url.protocol !== "https:") ||
    url.username !== "" ||
    url.password !== ""
  ) {
    throw new TypeError("page URL must be an absolute HTTP(S) URL");
  }
  return url;
}

function isYouTubeHost(hostname: string): boolean {
  const host = hostname.toLowerCase();
  return (
    host === "youtu.be" ||
    host === "youtube.com" ||
    host.endsWith(".youtube.com") ||
    host === "youtube-nocookie.com" ||
    host.endsWith(".youtube-nocookie.com")
  );
}

function firstWithheldReason(
  evidence: YouTubeMediaAuthorization,
): YouTubePolicyWithheldReason | null {
  assertConfirmation(evidence.servicePermission);
  assertConfirmation(evidence.mediaRights);
  assertConfirmation(evidence.lawfulUse);
  if (evidence.servicePermission === "prohibited") {
    return "service_prohibits_download";
  }
  if (evidence.servicePermission !== "confirmed") {
    return "service_permission_required";
  }
  if (evidence.mediaRights === "prohibited") return "media_rights_missing";
  if (evidence.mediaRights !== "confirmed") {
    return "media_rights_confirmation_required";
  }
  if (evidence.lawfulUse === "prohibited") {
    return "lawful_use_not_permitted";
  }
  return evidence.lawfulUse === "confirmed"
    ? null
    : "lawful_use_confirmation_required";
}

function assertConfirmation(value: YouTubePolicyConfirmation): void {
  if (
    value !== "confirmed" &&
    value !== "not_confirmed" &&
    value !== "prohibited"
  ) {
    throw new TypeError("YouTube policy confirmation is invalid");
  }
}
