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
  readonly listeners = new Map<string, Set<(event?: unknown) => void>>();
  readonly style: Record<string, string> = {};
  readonly shadow = new FakeNode();
  type = "";
  className = "";
  hidden = false;

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

  addEventListener(type: string, listener: (event?: unknown) => void): void {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: (event?: unknown) => void): void {
    this.listeners.get(type)?.delete(listener);
  }

  click(): void {
    for (const listener of this.listeners.get("click") ?? []) listener();
  }

  dispatch(type: string, event: unknown): void {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }

  getBoundingClientRect(): DOMRect {
    const left = Number.parseFloat(this.style.left ?? "0");
    const top = Number.parseFloat(this.style.top ?? "0");
    return { left, top } as DOMRect;
  }
}

class FakeDocument {
  readonly documentElement = new FakeElement("html");
  readonly created: FakeElement[] = [];
  readonly listeners = new Map<string, Set<(event?: unknown) => void>>();

  createElement(tagName: string): FakeElement {
    const element = new FakeElement(tagName);
    this.created.push(element);
    return element;
  }

  addEventListener(type: string, listener: (event?: unknown) => void): void {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: (event?: unknown) => void): void {
    this.listeners.get(type)?.delete(listener);
  }

  dispatch(type: string, event: unknown): void {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

function domFixture() {
  const document = new FakeDocument();
  const player = {
    isConnected: true,
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
      "−",
      "↕",
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

  it("minimizes and drags from isolated controls while reporting state", () => {
    const { document, player } = domFixture();
    const states: unknown[] = [];
    mountBasicMediaPlayerOverlay({
      document: document as unknown as Document,
      player: player as unknown as Element,
      onDownload: vi.fn(),
      initialInteractionState: {
        minimized: true,
        position: { x: 24, y: 36 },
      },
      onInteractionChange: (state) => states.push(state),
    });
    const host = document.documentElement.children[0] as FakeElement;
    const download = host.shadow.children[1] as FakeElement;
    const minimize = host.shadow.children[2] as FakeElement;
    const drag = host.shadow.children[3] as FakeElement;
    expect(download.hidden).toBe(true);
    expect(host.style).toMatchObject({
      left: "24px",
      top: "36px",
      transform: "none",
    });

    minimize.click();
    expect(download.hidden).toBe(false);
    expect(states).toEqual([{ minimized: false, position: { x: 24, y: 36 } }]);

    const preventDefault = vi.fn();
    drag.dispatch("pointerdown", {
      button: 0,
      pointerId: 7,
      clientX: 10,
      clientY: 20,
      preventDefault,
    });
    document.dispatch("pointermove", {
      pointerId: 7,
      clientX: 50,
      clientY: 70,
    });
    document.dispatch("pointerup", { pointerId: 7 });
    expect(preventDefault).toHaveBeenCalledOnce();
    expect(host.style).toMatchObject({ left: "64px", top: "86px" });
    expect(states.at(-1)).toEqual({
      minimized: false,
      position: { x: 64, y: 86 },
    });
  });
});
