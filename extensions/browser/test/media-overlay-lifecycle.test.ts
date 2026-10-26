import { describe, expect, it, vi } from "vitest";

import { installMediaOverlayLifecycle } from "../src/media-overlay-lifecycle.js";
import { MEDIA_PLAYER_OVERLAY_HOST_ATTRIBUTE } from "../src/media-player-overlay.js";

type Listener = () => void;

class FakeEventTarget {
  readonly listeners = new Map<string, Set<Listener>>();

  addEventListener(type: string, listener: Listener): void {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  dispatch(type: string): void {
    for (const listener of this.listeners.get(type) ?? []) listener();
  }
}

class FakeNode extends FakeEventTarget {
  readonly children: FakeNode[] = [];
  parent: FakeNode | null = null;
  textContent: string | null = null;
  root = false;

  get isConnected(): boolean {
    return this.root || this.parent?.isConnected === true;
  }

  append(...nodes: FakeNode[]): void {
    for (const node of nodes) {
      node.remove();
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
  readonly style: Record<string, string> = {};
  readonly shadow = new FakeNode();
  type = "";
  rect = rectFixture(40, 640, 620, 360);

  constructor(readonly tagName: string) {
    super();
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  attachShadow(): FakeNode {
    return this.shadow;
  }

  contains(candidate: unknown): boolean {
    if (candidate === this) return true;
    return this.children.some(
      (child) =>
        child === candidate ||
        (child instanceof FakeElement && child.contains(candidate)),
    );
  }

  getBoundingClientRect(): DOMRect {
    return this.rect as DOMRect;
  }
}

class FakeDocument extends FakeEventTarget {
  readonly documentElement = new FakeElement("html");
  readonly created: FakeElement[] = [];
  fullscreenElement: FakeElement | null = null;

  constructor() {
    super();
    this.documentElement.root = true;
  }

  createElement(tagName: string): FakeElement {
    const element = new FakeElement(tagName);
    this.created.push(element);
    return element;
  }
}

function rectFixture(
  top: number,
  right: number,
  width: number,
  height: number,
) {
  return {
    top,
    right,
    bottom: top + height,
    left: right - width,
    width,
    height,
    x: right - width,
    y: top,
    toJSON: () => ({}),
  };
}

function lifecycleFixture() {
  const document = new FakeDocument();
  const viewport = new FakeEventTarget();
  const events = new FakeEventTarget();
  const mutationCallbacks: Listener[] = [];
  const resizeCallbacks: Listener[] = [];
  const observed = new Set<FakeElement>();
  const frames = new Map<number, FrameRequestCallback>();
  let frameId = 0;
  let mutationsDisconnected = false;
  let resizesDisconnected = false;
  const windowFixture = {
    location: { href: "https://watch.example.test/episode/one#start" },
    visualViewport: viewport,
    MutationObserver: class {
      constructor(callback: MutationCallback) {
        mutationCallbacks.push(() =>
          callback([], this as unknown as MutationObserver),
        );
      }

      observe(): void {}
      disconnect(): void {
        mutationsDisconnected = true;
      }
      takeRecords(): MutationRecord[] {
        return [];
      }
    },
    ResizeObserver: class {
      constructor(callback: ResizeObserverCallback) {
        resizeCallbacks.push(() =>
          callback([], this as unknown as ResizeObserver),
        );
      }

      observe(player: Element): void {
        observed.add(player as unknown as FakeElement);
      }

      unobserve(player: Element): void {
        observed.delete(player as unknown as FakeElement);
      }

      disconnect(): void {
        observed.clear();
        resizesDisconnected = true;
      }
    },
    addEventListener: events.addEventListener.bind(events),
    removeEventListener: events.removeEventListener.bind(events),
    requestAnimationFrame(callback: FrameRequestCallback): number {
      frameId += 1;
      frames.set(frameId, callback);
      return frameId;
    },
    cancelAnimationFrame(id: number): void {
      frames.delete(id);
    },
  };
  const players: FakeElement[] = [];

  function addPlayer(tagName = "video"): FakeElement {
    const player = new FakeElement(tagName);
    document.documentElement.append(player);
    players.push(player);
    return player;
  }

  function removePlayer(player: FakeElement): void {
    player.remove();
    players.splice(players.indexOf(player), 1);
  }

  function flushFrames(): void {
    const queued = [...frames.entries()];
    frames.clear();
    for (const [, callback] of queued) callback(1);
  }

  function overlays(parent = document.documentElement): FakeElement[] {
    return parent.children.filter(
      (node): node is FakeElement =>
        node instanceof FakeElement &&
        node.attributes.has(MEDIA_PLAYER_OVERLAY_HOST_ATTRIBUTE),
    );
  }

  return {
    document,
    windowFixture,
    players,
    observed,
    addPlayer,
    removePlayer,
    overlays,
    flushFrames,
    mutate: () => mutationCallbacks[0]?.(),
    resizePlayer: () => resizeCallbacks[0]?.(),
    resizeWindow: () => events.dispatch("resize"),
    popstate: () => events.dispatch("popstate"),
    zoomViewport: () => viewport.dispatch("resize"),
    observersDisconnected: () => mutationsDisconnected && resizesDisconnected,
  };
}

describe("media overlay lifecycle", () => {
  it("tracks dynamically inserted and removed eligible players", () => {
    const fixture = lifecycleFixture();
    const first = fixture.addPlayer();
    const onDownload = vi.fn();
    const lifecycle = installMediaOverlayLifecycle({
      document: fixture.document as unknown as Document,
      window: fixture.windowFixture as unknown as Window,
      players: () => fixture.players as unknown as readonly Element[],
      onDownload,
      MutationObserver: fixture.windowFixture.MutationObserver,
      ResizeObserver: fixture.windowFixture.ResizeObserver,
    });
    expect(fixture.overlays()).toHaveLength(1);
    expect(fixture.observed).toEqual(new Set([first]));

    const second = fixture.addPlayer("audio");
    fixture.mutate();
    fixture.mutate();
    fixture.flushFrames();
    expect(fixture.overlays()).toHaveLength(2);
    expect(fixture.observed).toEqual(new Set([first, second]));

    fixture.removePlayer(first);
    fixture.mutate();
    fixture.flushFrames();
    expect(fixture.overlays()).toHaveLength(1);
    expect(fixture.observed).toEqual(new Set([second]));
    lifecycle.destroy();
    expect(fixture.overlays()).toEqual([]);
    expect(fixture.observersDisconnected()).toBe(true);
  });

  it("recreates overlays across SPA document identity changes", () => {
    const fixture = lifecycleFixture();
    fixture.addPlayer();
    installMediaOverlayLifecycle({
      document: fixture.document as unknown as Document,
      window: fixture.windowFixture as unknown as Window,
      players: () => fixture.players as unknown as readonly Element[],
      onDownload: vi.fn(),
      MutationObserver: fixture.windowFixture.MutationObserver,
      ResizeObserver: fixture.windowFixture.ResizeObserver,
    });
    const original = fixture.overlays()[0];

    fixture.windowFixture.location.href =
      "https://watch.example.test/episode/one#credits";
    fixture.popstate();
    fixture.flushFrames();
    expect(fixture.overlays()[0]).toBe(original);

    fixture.windowFixture.location.href =
      "https://watch.example.test/episode/two";
    fixture.mutate();
    fixture.flushFrames();
    expect(fixture.overlays()).toHaveLength(1);
    expect(fixture.overlays()[0]).not.toBe(original);
    expect(original?.parent).toBeNull();
  });

  it("repositions for theater, zoom, and element resize signals", () => {
    const fixture = lifecycleFixture();
    const player = fixture.addPlayer();
    installMediaOverlayLifecycle({
      document: fixture.document as unknown as Document,
      window: fixture.windowFixture as unknown as Window,
      players: () => fixture.players as unknown as readonly Element[],
      onDownload: vi.fn(),
      MutationObserver: fixture.windowFixture.MutationObserver,
      ResizeObserver: fixture.windowFixture.ResizeObserver,
    });
    const overlay = fixture.overlays()[0];
    expect(overlay?.style).toMatchObject({ top: "48px", left: "632px" });

    player.rect = rectFixture(12, 980, 960, 540);
    fixture.mutate();
    fixture.flushFrames();
    expect(overlay?.style).toMatchObject({ top: "20px", left: "972px" });

    player.rect = rectFixture(24, 720, 700, 394);
    fixture.zoomViewport();
    fixture.resizeWindow();
    fixture.resizePlayer();
    fixture.flushFrames();
    expect(overlay?.style).toMatchObject({ top: "32px", left: "712px" });
  });

  it("moves the overlay into the active fullscreen player container", () => {
    const fixture = lifecycleFixture();
    const fullscreen = new FakeElement("section");
    fixture.document.documentElement.append(fullscreen);
    const player = fixture.addPlayer();
    fullscreen.append(player);
    const lifecycle = installMediaOverlayLifecycle({
      document: fixture.document as unknown as Document,
      window: fixture.windowFixture as unknown as Window,
      players: () => fixture.players as unknown as readonly Element[],
      onDownload: vi.fn(),
      MutationObserver: fixture.windowFixture.MutationObserver,
      ResizeObserver: fixture.windowFixture.ResizeObserver,
    });
    const overlay = fixture.overlays()[0];

    fixture.document.fullscreenElement = fullscreen;
    fixture.document.dispatch("fullscreenchange");
    fixture.flushFrames();
    expect(fixture.overlays()).toEqual([]);
    expect(fixture.overlays(fullscreen)).toEqual([overlay]);

    fixture.document.fullscreenElement = null;
    fixture.document.dispatch("fullscreenchange");
    fixture.flushFrames();
    expect(fixture.overlays()).toEqual([overlay]);
    lifecycle.destroy();
  });
});
