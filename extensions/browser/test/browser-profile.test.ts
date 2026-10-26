import { describe, expect, it } from "vitest";

import {
  BrowserProfileMetadataBoundary,
  BrowserProfilePrivacyBoundary,
  createBrowserProfileScope,
  type BrowserProfileScope,
  type ProfileMetadataPort,
} from "../src/browser-profile.js";

class MemoryProfileMetadata<Metadata> implements ProfileMetadataPort<Metadata> {
  readonly values = new Map<string, Metadata>();
  loadCount = 0;
  saveCount = 0;

  public async load(scope: BrowserProfileScope): Promise<Metadata | null> {
    this.loadCount += 1;
    return this.values.get(scope.key) ?? null;
  }

  public async save(
    scope: BrowserProfileScope,
    metadata: Metadata,
  ): Promise<void> {
    this.saveCount += 1;
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

describe("incognito profile privacy boundary", () => {
  function fixture() {
    const port = new MemoryProfileMetadata<{ cookieReference: string }>();
    const profile = new BrowserProfileMetadataBoundary(
      { browser: "chrome", profileId: "profile_personal_0001" },
      port,
    );
    return {
      port,
      privacy: new BrowserProfilePrivacyBoundary(profile),
    };
  }

  it("is default-off and requires explicit session opt-in", async () => {
    const { port, privacy } = fixture();

    expect(privacy.incognitoEnabled).toBe(false);
    await expect(
      privacy.save("incognito", { cookieReference: "private-ref" }),
    ).rejects.toThrow("incognito integration is disabled");
    await expect(privacy.load("incognito")).rejects.toThrow(
      "incognito integration is disabled",
    );
    expect(port.loadCount).toBe(0);
    expect(port.saveCount).toBe(0);
  });

  it("keeps enabled private metadata ephemeral and clears it at session end", async () => {
    const { port, privacy } = fixture();

    privacy.enableIncognitoSession();
    await privacy.save("incognito", { cookieReference: "private-ref" });
    expect(await privacy.load("incognito")).toEqual({
      cookieReference: "private-ref",
    });
    expect(port.values).toHaveLength(0);
    expect(port.loadCount).toBe(0);
    expect(port.saveCount).toBe(0);

    privacy.endIncognitoSession();
    privacy.enableIncognitoSession();
    expect(await privacy.load("incognito")).toBeNull();
  });

  it("persists only regular metadata through the profile boundary", async () => {
    const { port, privacy } = fixture();

    await privacy.save("regular", { cookieReference: "regular-ref" });
    expect(await privacy.load("regular")).toEqual({
      cookieReference: "regular-ref",
    });
    expect(port.saveCount).toBe(1);
    expect(port.loadCount).toBe(1);
  });
});
