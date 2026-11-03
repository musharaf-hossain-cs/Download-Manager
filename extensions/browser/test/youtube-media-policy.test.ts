import { describe, expect, it } from "vitest";

import {
  YOUTUBE_DOWNLOAD_SCOPE_NOTICE,
  evaluateMediaDownloadOffer,
  type YouTubeMediaAuthorization,
} from "../src/youtube-media-policy.js";

const CONFIRMED: YouTubeMediaAuthorization = {
  servicePermission: "confirmed",
  mediaRights: "confirmed",
  lawfulUse: "confirmed",
};

describe("YouTube media offer policy", () => {
  it("fails closed with a stable scope notice when no authorization is supplied", () => {
    expect(
      evaluateMediaDownloadOffer("https://www.youtube.com/watch?v=example"),
    ).toEqual({
      offer: false,
      site: "youtube",
      reason: "service_permission_required",
      explanation:
        "Download action withheld: confirm that the applicable YouTube service permission or terms allow this download.",
      notice: YOUTUBE_DOWNLOAD_SCOPE_NOTICE,
    });
    expect(YOUTUBE_DOWNLOAD_SCOPE_NOTICE).toContain(
      "YouTube downloads are not universally available",
    );
  });

  it("offers the action only after service, rights, and lawful-use confirmation", () => {
    const cases: readonly [Partial<YouTubeMediaAuthorization>, string][] = [
      [{ servicePermission: "prohibited" }, "service_prohibits_download"],
      [
        { servicePermission: "confirmed" },
        "media_rights_confirmation_required",
      ],
      [
        { servicePermission: "confirmed", mediaRights: "prohibited" },
        "media_rights_missing",
      ],
      [
        { servicePermission: "confirmed", mediaRights: "confirmed" },
        "lawful_use_confirmation_required",
      ],
      [
        {
          servicePermission: "confirmed",
          mediaRights: "confirmed",
          lawfulUse: "prohibited",
        },
        "lawful_use_not_permitted",
      ],
    ];
    for (const [override, reason] of cases) {
      const decision = evaluateMediaDownloadOffer("https://youtu.be/example", {
        servicePermission: "not_confirmed",
        mediaRights: "not_confirmed",
        lawfulUse: "not_confirmed",
        ...override,
      });
      expect(decision).toMatchObject({ offer: false, reason });
    }

    expect(
      evaluateMediaDownloadOffer(
        "https://music.youtube.com/watch?v=example",
        CONFIRMED,
      ),
    ).toEqual({
      offer: true,
      site: "youtube",
      notice: YOUTUBE_DOWNLOAD_SCOPE_NOTICE,
    });
  });

  it("does not confuse lookalike hosts with YouTube service pages", () => {
    for (const url of [
      "https://notyoutube.com/watch/example",
      "https://youtube.com.attacker.example/watch/example",
      "https://watch.example.test/video/example",
    ]) {
      expect(evaluateMediaDownloadOffer(url)).toEqual({
        offer: true,
        site: "other",
        notice: null,
      });
    }
    expect(
      evaluateMediaDownloadOffer(
        "https://www.youtube-nocookie.com/embed/example",
        CONFIRMED,
      ).site,
    ).toBe("youtube");
  });

  it("rejects non-HTTP pages and malformed runtime confirmation values", () => {
    expect(() => evaluateMediaDownloadOffer("file:///video.html")).toThrow(
      "page URL must be an absolute HTTP(S) URL",
    );
    expect(() =>
      evaluateMediaDownloadOffer("https://youtube.com/watch?v=example", {
        ...CONFIRMED,
        servicePermission: "maybe",
      } as unknown as YouTubeMediaAuthorization),
    ).toThrow("YouTube policy confirmation is invalid");
  });
});
