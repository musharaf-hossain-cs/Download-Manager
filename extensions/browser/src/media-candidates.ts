import { normalizeHttpUrl } from "./http-url.js";

export const MAX_MEDIA_CANDIDATES = 256;

export type MediaCandidateKind = "progressive" | "hls" | "dash";
export type MediaCandidateSource =
  "media-element" | "source-element" | "network-response";

export interface MediaCandidateObservation {
  readonly url: string;
  readonly mimeType?: string | null;
  readonly playerId?: string | null;
  readonly source: MediaCandidateSource;
}

export interface MediaCandidate {
  readonly url: string;
  readonly kind: MediaCandidateKind;
  readonly mimeType: string | null;
  readonly playerIds: readonly string[];
  readonly sources: readonly MediaCandidateSource[];
}

export type MediaCandidateObservationResult =
  | {
      readonly status: "accepted" | "merged";
      readonly candidate: MediaCandidate;
    }
  | {
      readonly status: "rejected";
      readonly reason:
        | "invalid_url"
        | "invalid_mime_type"
        | "invalid_player_id"
        | "unsupported_type"
        | "conflicting_type"
        | "candidate_limit";
    };

export interface MediaCandidateSnapshot {
  readonly pageUrl: string;
  readonly candidates: readonly MediaCandidate[];
}

interface MutableCandidate {
  readonly url: string;
  readonly kind: MediaCandidateKind;
  mimeType: string | null;
  readonly playerIds: Set<string>;
  readonly sources: Set<MediaCandidateSource>;
}

const HLS_MIME_TYPES = new Set([
  "application/vnd.apple.mpegurl",
  "application/x-mpegurl",
  "audio/mpegurl",
  "audio/x-mpegurl",
]);
const DASH_MIME_TYPES = new Set(["application/dash+xml"]);
const PROGRESSIVE_MIME_TYPES = new Set([
  "video/mp4",
  "video/webm",
  "video/x-matroska",
  "audio/mpeg",
  "audio/mp4",
  "audio/aac",
  "audio/ogg",
  "video/ogg",
]);
const GENERIC_MIME_TYPES = new Set([
  "application/octet-stream",
  "binary/octet-stream",
]);
const PROGRESSIVE_EXTENSIONS = new Set([
  "mp4",
  "webm",
  "mkv",
  "mp3",
  "m4a",
  "aac",
  "ogg",
  "oga",
  "ogv",
]);

/** Page-scoped, bounded media candidate model shared by player detectors. */
export class MediaCandidateModel {
  readonly #candidates = new Map<string, MutableCandidate>();
  #pageUrl: string;

  constructor(pageUrl: string) {
    this.#pageUrl = normalizePageUrl(pageUrl);
  }

  /** Clears observations when an SPA moves to a different document identity. */
  navigate(pageUrl: string): boolean {
    const normalized = normalizePageUrl(pageUrl);
    if (normalized === this.#pageUrl) return false;
    this.#pageUrl = normalized;
    this.#candidates.clear();
    return true;
  }

  observe(
    observation: MediaCandidateObservation,
  ): MediaCandidateObservationResult {
    const url = canonicalMediaUrl(observation.url, this.#pageUrl);
    if (url === null) return { status: "rejected", reason: "invalid_url" };

    const mime = normalizeMimeType(observation.mimeType);
    if (mime === undefined) {
      return { status: "rejected", reason: "invalid_mime_type" };
    }
    if (!validPlayerId(observation.playerId)) {
      return { status: "rejected", reason: "invalid_player_id" };
    }

    const kind = classifyCandidate(url, mime);
    if (kind === null) {
      return { status: "rejected", reason: "unsupported_type" };
    }

    const existing = this.#candidates.get(url);
    if (existing !== undefined) {
      if (existing.kind !== kind) {
        return { status: "rejected", reason: "conflicting_type" };
      }
      existing.sources.add(observation.source);
      if (observation.playerId != null) {
        existing.playerIds.add(observation.playerId);
      }
      existing.mimeType ??= mime;
      return { status: "merged", candidate: immutableCandidate(existing) };
    }
    if (this.#candidates.size === MAX_MEDIA_CANDIDATES) {
      return { status: "rejected", reason: "candidate_limit" };
    }

    const candidate: MutableCandidate = {
      url,
      kind,
      mimeType: mime,
      playerIds: new Set(
        observation.playerId == null ? [] : [observation.playerId],
      ),
      sources: new Set([observation.source]),
    };
    this.#candidates.set(url, candidate);
    return { status: "accepted", candidate: immutableCandidate(candidate) };
  }

  snapshot(): MediaCandidateSnapshot {
    return Object.freeze({
      pageUrl: this.#pageUrl,
      candidates: Object.freeze(
        [...this.#candidates.values()].map(immutableCandidate),
      ),
    });
  }
}

function normalizePageUrl(value: string): string {
  const normalized = normalizeHttpUrl(value);
  if (normalized === null) {
    throw new TypeError("page URL must be an absolute HTTP(S) URL");
  }
  const url = new URL(normalized);
  url.hash = "";
  return url.toString();
}

function canonicalMediaUrl(value: string, pageUrl: string): string | null {
  const normalized = normalizeHttpUrl(value, pageUrl);
  if (normalized === null) return null;
  const url = new URL(normalized);
  url.hash = "";
  return url.toString();
}

function normalizeMimeType(
  value: string | null | undefined,
): string | null | undefined {
  if (value == null || value.trim() === "") return null;
  const normalized = value.split(";", 1)[0]?.trim().toLowerCase();
  return normalized !== undefined && /^[^\s/]+\/[^\s/]+$/u.test(normalized)
    ? normalized
    : undefined;
}

function validPlayerId(value: string | null | undefined): boolean {
  return value == null || (value.length > 0 && value.length <= 256);
}

function classifyCandidate(
  url: string,
  mimeType: string | null,
): MediaCandidateKind | null {
  if (mimeType !== null) {
    if (HLS_MIME_TYPES.has(mimeType)) return "hls";
    if (DASH_MIME_TYPES.has(mimeType)) return "dash";
    if (PROGRESSIVE_MIME_TYPES.has(mimeType)) return "progressive";
    if (!GENERIC_MIME_TYPES.has(mimeType)) return null;
  }

  const extension = new URL(url).pathname
    .split("/")
    .at(-1)
    ?.match(/\.([^.]+)$/u)?.[1]
    ?.toLowerCase();
  if (extension === "m3u8") return "hls";
  if (extension === "mpd") return "dash";
  return extension !== undefined && PROGRESSIVE_EXTENSIONS.has(extension)
    ? "progressive"
    : null;
}

function immutableCandidate(candidate: MutableCandidate): MediaCandidate {
  return Object.freeze({
    url: candidate.url,
    kind: candidate.kind,
    mimeType: candidate.mimeType,
    playerIds: Object.freeze([...candidate.playerIds]),
    sources: Object.freeze([...candidate.sources]),
  });
}
