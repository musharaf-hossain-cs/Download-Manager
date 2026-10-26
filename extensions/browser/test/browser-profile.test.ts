import { describe, expect, it } from "vitest";

import {
  BrowserProfileMetadataBoundary,
  createBrowserProfileScope,
  type BrowserProfileScope,
  type ProfileMetadataPort,
} from "../src/browser-profile.js";

class MemoryProfileMetadata<Metadata> implements ProfileMetadataPort<Metadata> {
  readonly values = new Map<string, Metadata>();

  public async load(scope: BrowserProfileScope): Promise<Metadata | null> {
    return this.values.get(scope.key) ?? null;
  }

  public async save(
    scope: BrowserProfileScope,
    metadata: Metadata,
  ): Promise<void> {
    this.values.set(scope.key, metadata);
  }
}

describe("browser profile identity boundary", () => {
  it("never mixes metadata from two browser profiles", async () => {
    const port = new MemoryProfileMetadata<{ cookieReference: string }>();
    const personal = new BrowserProfileMetadataBoundary(
      {
        browser: "chrome",
        profileId: "profile_personal_0001",
      },
      port,
    );
    const work = new BrowserProfileMetadataBoundary(
      {
        browser: "chrome",
        profileId: "profile_work_00000001",
      },
      port,
    );

    await personal.save({ cookieReference: "personal-secret-ref" });
    await work.save({ cookieReference: "work-secret-ref" });

    expect(await personal.load()).toEqual({
      cookieReference: "personal-secret-ref",
    });
    expect(await work.load()).toEqual({
      cookieReference: "work-secret-ref",
    });
    expect(port.values).toHaveLength(2);
  });

  it("includes the browser in the scope and rejects forgeable IDs", () => {
    const chrome = createBrowserProfileScope({
      browser: "chrome",
      profileId: "shared_profile_0001",
    });
    const edge = createBrowserProfileScope({
      browser: "edge",
      profileId: "shared_profile_0001",
    });

    expect(chrome.key).not.toBe(edge.key);
    expect(chrome).toEqual({
      browser: "chrome",
      profileId: "shared_profile_0001",
      key: '["chrome","shared_profile_0001"]',
    });
    expect(() =>
      createBrowserProfileScope({
        browser: "chrome",
        profileId: "too-short",
      }),
    ).toThrow("invalid browser profile ID");
    expect(() =>
      createBrowserProfileScope({
        browser: "safari" as "chrome",
        profileId: "profile_personal_0001",
      }),
    ).toThrow("unsupported browser identity");
  });
});
