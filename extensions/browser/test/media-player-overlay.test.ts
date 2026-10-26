import { describe, expect, it, vi } from "vitest";

import {
  MEDIA_PLAYER_OVERLAY_HOST_ATTRIBUTE,
  mountBasicMediaPlayerOverlay,
} from "../src/media-player-overlay.js";

class FakeNode {
  readonly children: FakeNode[] = [];
  parent: FakeNode | null = null;
  textContent: string | null = null;

  append(...nodes: FakeNode[]): void {
    for (const node of nodes) {
      node.parent = this;
      this.children.push(node);
    }
  }

  remove(): void {
    if (this.parent === null) return;
    const index = this.parent.children.indexOf(this);
    if (index >= 0) this.parent.children.splice(index, 1);
    this.parent = null;
  }
}

class FakeElement extends FakeNode {
  readonly attributes = new Map<string, string>();
  readonly listeners = new Map<string, Set<() => void>>();
  readonly style: Record<string, string> = {};
  readonly shadow = new FakeNode();
  type = "";

  constructor(readonly tagName: string) {
    super();
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  attachShadow(options: ShadowRootInit): FakeNode {
    expect(options).toEqual({ mode: "closed" });
    return this.shadow;
  }

  addEventListener(type: string, listener: () => void): void {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: () => void): void {
    this.listeners.get(type)?.delete(listener);
  }

  click(): void {
    for (const listener of this.listeners.get("click") ?? []) listener();
  }
}

class FakeDocument {
  readonly documentElement = new FakeElement("html");
  readonly created: FakeElement[] = [];

  createElement(tagName: string): FakeElement {
    const element = new FakeElement(tagName);
    this.created.push(element);
    return element;
  }
}

function domFixture() {
  const document = new FakeDocument();
  const player = {
    controls: true,
    className: "site-player theater",
    style: { position: "relative", width: "100%" },
    getBoundingClientRect: () => ({
      top: 40,
      right: 640,
      bottom: 400,
      left: 20,
      width: 620,
      height: 360,
      x: 20,
      y: 40,
      toJSON: () => ({}),
    }),
  };
  return { document, player };
}

describe("basic media player overlay", () => {
  it("mounts an isolated, player-anchored Shadow DOM button", () => {
    const { document, player } = domFixture();
    const onDownload = vi.fn();
    const overlay = mountBasicMediaPlayerOverlay({
      document: document as unknown as Document,
      player: player as unknown as Element,
      onDownload,
    });

    const host = document.documentElement.children[0] as FakeElement;
    expect(host.attributes.get(MEDIA_PLAYER_OVERLAY_HOST_ATTRIBUTE)).toBe("");
    expect(host.style).toMatchObject({
      position: "fixed",
      top: "48px",
      left: "632px",
      transform: "translateX(-100%)",
      pointerEvents: "none",
    });
    expect(host.shadow.children.map((node) => node.textContent)).toEqual([
      expect.stringContaining(":host { all: initial; }"),
      "Download media",
    ]);
    const button = host.shadow.children[1] as FakeElement;
    expect(button.type).toBe("button");
    expect(button.attributes.get("aria-label")).toBe("Download media");
    button.click();
    expect(onDownload).toHaveBeenCalledOnce();

    overlay.destroy();
    overlay.destroy();
    expect(document.documentElement.children).toEqual([]);
    button.click();
    expect(onDownload).toHaveBeenCalledOnce();
  });

  it("never mutates the host player's layout, classes, or controls", () => {
    const { document, player } = domFixture();
    const before = structuredClone({
      controls: player.controls,
      className: player.className,
      style: player.style,
    });

    const overlay = mountBasicMediaPlayerOverlay({
      document: document as unknown as Document,
      player: player as unknown as Element,
      onDownload: vi.fn(),
    });

    expect({
      controls: player.controls,
      className: player.className,
      style: player.style,
    }).toEqual(before);
    expect(document.documentElement.children).toEqual([overlay.host]);
  });

  it("keeps the button inside the viewport gutter near clipped players", () => {
    const { document, player } = domFixture();
    player.getBoundingClientRect = () => ({
      top: -100,
      right: 4,
      bottom: 4,
      left: -100,
      width: 104,
      height: 104,
      x: -100,
      y: -100,
      toJSON: () => ({}),
    });

    mountBasicMediaPlayerOverlay({
      document: document as unknown as Document,
      player: player as unknown as Element,
      onDownload: vi.fn(),
    });

    expect(
      (document.documentElement.children[0] as FakeElement | undefined)?.style,
    ).toMatchObject({
      top: "8px",
      left: "8px",
    });
  });
});
