import { describe, expect, it } from "vitest";

import {
  DEFAULT_CAPTURE_MODIFIERS,
  LinkHandoffController,
  MediaOverlayInteractionController,
  SINGLE_LINK_CONTEXT_MENU_ID,
  collectAllLinks,
  collectSelectedLinks,
  evaluateCaptureRules,
  evaluateSiteCapture,
  normalizeSingleLinkClick,
  resolveCaptureModifier,
} from "../src/index.js";
import {
  CHROMIUM_P1_MATRIX,
  CHROMIUM_P1_SCENARIOS,
  createChromiumP1Report,
  serializeChromiumP1Report,
} from "../../../scripts/chromium-p1-regression.mjs";

const requestId = "018f47ce-8e51-7f31-a1ec-4a97f84a12cd";
const jobId = "018f47ce-8e51-7f31-a1ec-4a97f84a12ce";
const extensionId = "ackbonlbpniilcfmmnfdohnhbdbajhii";
const none = Object.freeze({
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
});

function handoffController(health, events, fallbacks) {
  return new LinkHandoffController(
    health,
    {
      async send(request) {
        events.push("native:send");
        return {
          version: 1,
          request_id: request.request_id,
          payload: { type: "accepted", job_ids: [jobId] },
        };
      },
    },
    {
      async cancel(id) {
        events.push(`cancel:${String(id)}`);
      },
      async fallback(details) {
        fallbacks.push(details.url);
      },
    },
    () => ({ requestId, nonce: "abcdefghijklmnop", extensionId }),
  );
}

async function handoffContract() {
  const candidate = {
    downloadId: 42,
    url: "https://files.example.test/archive.zip?token=signed",
    referrer: "https://www.example.test/releases",
    suggestedFilename: "archive.zip",
  };
  const events = [];
  const accepted = handoffController({ state: "connected" }, events, []);
  const result = await accepted.handoff(candidate);
  const duplicate = await accepted.handoff(candidate);

  const fallbacks = [];
  const offlineEvents = [];
  const offline = handoffController(
    { state: "disconnected" },
    offlineEvents,
    fallbacks,
  );
  const fallback = await offline.handoff({ ...candidate, downloadId: 7 });
  return { result, duplicate, events, fallback, fallbacks, offlineEvents };
}

function captureContract() {
  const captureRules = {
    extensions: ["zip"],
    mimeTypes: ["application/pdf"],
    minimumSizeBytes: 1_024,
  };
  const siteRules = {
    globalPolicy: "deny",
    sites: [{ hostPattern: "*.example.test", policy: "allow" }],
  };
  const candidate = {
    url: "https://cdn.example.test/archive.zip",
    suggestedFilename: "archive.zip",
    mimeType: "application/zip",
    sizeBytes: 4_096,
  };
  const decide = (value, modifiers) => {
    const action = resolveCaptureModifier(
      modifiers,
      DEFAULT_CAPTURE_MODIFIERS,
    ).action;
    if (action === "bypass") return false;
    if (action === "force") return true;
    return (
      evaluateSiteCapture(value.url, siteRules).capture &&
      evaluateCaptureRules(value, captureRules).capture
    );
  };
  const denied = { ...candidate, url: "https://blocked.test/readme.txt" };
  return {
    allowed: decide(candidate, none),
    denied: decide(denied, none),
    forced: decide(denied, { ...none, ctrlKey: true }),
    bypassed: decide(candidate, { ...none, altKey: true }),
    conflict: decide(candidate, { ...none, altKey: true, ctrlKey: true }),
  };
}

function documentFixture(values) {
  return {
    baseURI: "https://www.example.test/releases/index.html",
    querySelectorAll() {
      return values.map((value) => ({ getAttribute: () => value }));
    },
  };
}

async function contextContract() {
  const page = documentFixture([
    "../one.zip#first",
    "../one.zip#duplicate",
    "javascript:alert(1)",
    "//cdn.example.test/two.exe",
  ]);
  const single = normalizeSingleLinkClick({
    menuItemId: SINGLE_LINK_CONTEXT_MENU_ID,
    linkUrl: "../single.zip",
    pageUrl: page.baseURI,
  });
  const all = await collectAllLinks(page);
  const selected = await collectSelectedLinks({
    baseURI: page.baseURI,
    getSelection() {
      return {
        rangeCount: 1,
        getRangeAt() {
          return {
            cloneContents: () =>
              documentFixture(["../chosen.zip", "mailto:x@y"]),
          };
        },
      };
    },
  });
  return [single, all.items, selected.items];
}

async function overlayContract() {
  const saved = [];
  const controller = new MediaOverlayInteractionController(
    {},
    { save: async (snapshot) => saved.push(snapshot) },
  );
  const site = "https://watch.example.test/episode/one";
  await controller.setSiteEnabled(site, false);
  const disabled = controller.isEnabled(site);
  await controller.setSiteEnabled(site, true);
  await controller.setMinimized(site, true);
  await controller.move(
    site,
    { x: -20, y: 200 },
    { x: 8, y: 8, width: 120, height: 80 },
  );
  return {
    disabled,
    enabled: controller.isEnabled(site),
    state: controller.stateFor(site),
    saves: saved.length,
  };
}

describe("Brave and Opera current-stable shared Chromium regression", () => {
  for (const variant of CHROMIUM_P1_MATRIX) {
    const label = `${variant.browser}-${variant.version}`;

    it(`${label} preserves acknowledged handoff and offline fallback`, async () => {
      await expect(handoffContract()).resolves.toEqual({
        result: { kind: "accepted", jobId },
        duplicate: { kind: "ignored", reason: "duplicate" },
        events: ["native:send", "cancel:42"],
        fallback: { kind: "fallback", reason: "native_unavailable" },
        fallbacks: ["https://files.example.test/archive.zip?token=signed"],
        offlineEvents: [],
      });
    });

    it(`${label} applies capture rules and conflict-safe modifiers`, () => {
      expect(captureContract()).toEqual({
        allowed: true,
        denied: false,
        forced: true,
        bypassed: false,
        conflict: false,
      });
    });

    it(`${label} normalizes single, all, and selected context actions`, async () => {
      const [single, all, selected] = await contextContract();
      expect(single.url).toBe("https://www.example.test/single.zip");
      expect(all.map(({ url }) => url)).toEqual([
        "https://www.example.test/one.zip",
        "https://cdn.example.test/two.exe",
      ]);
      expect(selected.map(({ url }) => url)).toEqual([
        "https://www.example.test/chosen.zip",
      ]);
    });

    it(`${label} preserves site overlay enablement and bounded position`, async () => {
      await expect(overlayContract()).resolves.toEqual({
        disabled: false,
        enabled: true,
        state: { minimized: true, position: { x: 8, y: 88 } },
        saves: 4,
      });
    });
  }

  it("serializes exact BROW-03 evidence with no discovered continuation", () => {
    expect(CHROMIUM_P1_SCENARIOS).toHaveLength(4);
    expect(serializeChromiumP1Report()).toBe(serializeChromiumP1Report());
    expect(createChromiumP1Report()).toMatchObject({
      schema_version: 1,
      scope: "brave_opera_current_stable_adapter_fixture",
      requirement: "BROW-03",
      as_of: "2026-11-03",
      coverage: {
        execution: "deterministic_adapter_contract",
        browser_binaries_launched: false,
        distribution: "where_store_or_policy_permits",
      },
      browsers: [
        { browser: "brave", version: "1.98.63", chromium: "156.0.8078.80" },
        {
          browser: "opera",
          version: "139.0.6053.15",
          chromium: "155.0.8059.85",
        },
      ],
      isolated_continuations: [],
    });
  });
});
