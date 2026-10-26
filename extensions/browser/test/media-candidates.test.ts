import { describe, expect, it } from "vitest";

import {
  MAX_MEDIA_CANDIDATES,
  MediaCandidateModel,
  type MediaCandidateObservation,
} from "../src/media-candidates.js";

function observe(
  model: MediaCandidateModel,
  observation: Partial<MediaCandidateObservation> &
    Pick<MediaCandidateObservation, "url">,
) {
  return model.observe({
    source: "media-element",
    ...observation,
  });
}

describe("media candidate model", () => {
  it("classifies progressive, HLS, and DASH observations without trusting query text", () => {
    const model = new MediaCandidateModel(
      "https://watch.example.test/show/episode-1",
    );

    expect(observe(model, { url: "/media/movie.mp4#play" }).status).toBe(
      "accepted",
    );
    expect(
      observe(model, {
        url: "https://cdn.example.test/live?id=primary",
        mimeType: "application/vnd.apple.mpegurl; charset=UTF-8",
        source: "network-response",
      }).status,
    ).toBe("accepted");
    expect(
      observe(model, {
        url: "/manifest/main.mpd?token=short-lived",
        source: "source-element",
      }).status,
    ).toBe("accepted");
    expect(
      observe(model, {
        url: "/audio/commentary",
        mimeType: 'audio/mp4; codecs="mp4a.40.2"',
      }).status,
    ).toBe("accepted");

    expect(model.snapshot().candidates).toEqual([
      {
        url: "https://watch.example.test/media/movie.mp4",
        kind: "progressive",
        mimeType: null,
        playerIds: [],
        sources: ["media-element"],
      },
      {
        url: "https://cdn.example.test/live?id=primary",
        kind: "hls",
        mimeType: "application/vnd.apple.mpegurl",
        playerIds: [],
        sources: ["network-response"],
      },
      {
        url: "https://watch.example.test/manifest/main.mpd?token=short-lived",
        kind: "dash",
        mimeType: null,
        playerIds: [],
        sources: ["source-element"],
      },
      {
        url: "https://watch.example.test/audio/commentary",
        kind: "progressive",
        mimeType: "audio/mp4",
        playerIds: [],
        sources: ["media-element"],
      },
    ]);
  });

  it("deduplicates one candidate across multiple players and observation sources", () => {
    const model = new MediaCandidateModel("https://watch.example.test/");

    expect(
      observe(model, {
        url: "/asset/master.m3u8#video-one",
        playerId: "player-one",
      }).status,
    ).toBe("accepted");
    expect(
      observe(model, {
        url: "https://watch.example.test/asset/master.m3u8#video-two",
        playerId: "player-two",
        source: "source-element",
      }).status,
    ).toBe("merged");
    expect(
      observe(model, {
        url: "/asset/master.m3u8",
        mimeType: "application/x-mpegURL",
        source: "network-response",
      }).status,
    ).toBe("merged");

    expect(model.snapshot().candidates).toEqual([
      {
        url: "https://watch.example.test/asset/master.m3u8",
        kind: "hls",
        mimeType: "application/x-mpegurl",
        playerIds: ["player-one", "player-two"],
        sources: ["media-element", "source-element", "network-response"],
      },
    ]);
  });

  it("resets stale candidates on SPA navigation but not fragment-only changes", () => {
    const model = new MediaCandidateModel(
      "https://watch.example.test/show/one#opening",
    );
    observe(model, { url: "/media/one.webm", playerId: "hero" });

    expect(model.navigate("https://watch.example.test/show/one#credits")).toBe(
      false,
    );
    expect(model.snapshot().candidates).toHaveLength(1);
    expect(model.navigate("https://watch.example.test/show/two")).toBe(true);
    expect(model.snapshot()).toEqual({
      pageUrl: "https://watch.example.test/show/two",
      candidates: [],
    });

    observe(model, { url: "../media/two.mp4", playerId: "hero" });
    expect(model.snapshot().candidates[0]?.url).toBe(
      "https://watch.example.test/media/two.mp4",
    );
  });

  it("rejects false positives and malformed untrusted observations", () => {
    const model = new MediaCandidateModel("https://watch.example.test/");
    const cases: readonly [
      Partial<MediaCandidateObservation> & { url: string },
      string,
    ][] = [
      [{ url: "/poster.jpg" }, "unsupported_type"],
      [{ url: "/analytics?file=trailer.mp4" }, "unsupported_type"],
      [{ url: "/fake.mp4", mimeType: "text/html" }, "unsupported_type"],
      [{ url: "blob:https://watch.example.test/id" }, "invalid_url"],
      [{ url: "data:video/mp4;base64,AAAA" }, "invalid_url"],
      [{ url: "/movie.mp4", mimeType: "not-a-mime" }, "invalid_mime_type"],
      [{ url: "/movie.mp4", playerId: "" }, "invalid_player_id"],
    ];

    for (const [candidate, reason] of cases) {
      expect(observe(model, candidate)).toEqual({ status: "rejected", reason });
    }
    expect(model.snapshot().candidates).toEqual([]);
  });

  it("bounds page-owned candidate growth while allowing existing merges", () => {
    const model = new MediaCandidateModel("https://watch.example.test/");
    for (let index = 0; index < MAX_MEDIA_CANDIDATES; index += 1) {
      expect(
        observe(model, { url: `/media/${String(index)}.mp4` }).status,
      ).toBe("accepted");
    }

    expect(observe(model, { url: "/media/overflow.mp4" })).toEqual({
      status: "rejected",
      reason: "candidate_limit",
    });
    expect(
      observe(model, {
        url: "/media/0.mp4",
        playerId: "late-player",
        source: "network-response",
      }).status,
    ).toBe("merged");
    expect(model.snapshot().candidates).toHaveLength(MAX_MEDIA_CANDIDATES);
  });

  it("rejects conflicting classifications for one canonical URL", () => {
    const model = new MediaCandidateModel("https://watch.example.test/");
    observe(model, {
      url: "/opaque",
      mimeType: "video/mp4",
      playerId: "player",
    });

    expect(
      observe(model, {
        url: "/opaque",
        mimeType: "application/dash+xml",
        playerId: "player",
        source: "network-response",
      }),
    ).toEqual({ status: "rejected", reason: "conflicting_type" });
    expect(model.snapshot().candidates).toHaveLength(1);
    expect(model.snapshot().candidates[0]?.kind).toBe("progressive");
  });

  it("validates page scope and returns immutable snapshots", () => {
    expect(() => new MediaCandidateModel("file:///private/video.html")).toThrow(
      "absolute HTTP(S)",
    );
    const model = new MediaCandidateModel("https://watch.example.test/");
    observe(model, { url: "/movie.mp4" });
    const snapshot = model.snapshot();

    expect(Object.isFrozen(snapshot)).toBe(true);
    expect(Object.isFrozen(snapshot.candidates)).toBe(true);
    expect(Object.isFrozen(snapshot.candidates[0])).toBe(true);
  });
});
