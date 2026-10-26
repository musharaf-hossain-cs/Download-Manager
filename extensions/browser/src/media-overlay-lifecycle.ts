import {
  mountBasicMediaPlayerOverlay,
  type BasicMediaPlayerOverlay,
} from "./media-player-overlay.js";

export interface MediaOverlayLifecycle {
  refresh(): void;
  destroy(): void;
}

export interface MediaOverlayLifecycleOptions {
  readonly document: Document;
  readonly window: Window;
  /** Returns only players that have a downloadable candidate. */
  readonly players: () => readonly Element[];
  readonly onDownload: (player: Element) => Promise<void> | void;
  readonly MutationObserver?: typeof MutationObserver;
  readonly ResizeObserver?: typeof ResizeObserver;
}

/** Keeps candidate-backed overlays aligned with a dynamic media document. */
export function installMediaOverlayLifecycle(
  options: MediaOverlayLifecycleOptions,
): MediaOverlayLifecycle {
  const { document, window, players, onDownload } = options;
  const MutationObserverConstructor =
    options.MutationObserver ?? globalThis.MutationObserver;
  const ResizeObserverConstructor =
    options.ResizeObserver ?? globalThis.ResizeObserver;
  const overlays = new Map<Element, BasicMediaPlayerOverlay>();
  let pageIdentity = documentIdentity(window.location.href);
  let frame: number | null = null;
  let destroyed = false;

  const resizeObserver = new ResizeObserverConstructor(() => schedule());

  const removePlayer = (player: Element): void => {
    const overlay = overlays.get(player);
    if (overlay === undefined) return;
    resizeObserver.unobserve(player);
    overlay.destroy();
    overlays.delete(player);
  };

  const removeAll = (): void => {
    for (const player of [...overlays.keys()]) removePlayer(player);
  };

  const refresh = (): void => {
    if (destroyed) return;
    if (frame !== null) {
      window.cancelAnimationFrame(frame);
      frame = null;
    }

    const nextIdentity = documentIdentity(window.location.href);
    if (nextIdentity !== pageIdentity) {
      pageIdentity = nextIdentity;
      removeAll();
    }

    const currentPlayers = new Set(
      players().filter((player) => player.isConnected),
    );
    for (const player of [...overlays.keys()]) {
      if (!currentPlayers.has(player)) removePlayer(player);
    }

    for (const player of currentPlayers) {
      let overlay = overlays.get(player);
      if (overlay === undefined) {
        overlay = mountBasicMediaPlayerOverlay({
          document,
          player,
          onDownload: () => onDownload(player),
        });
        overlays.set(player, overlay);
        resizeObserver.observe(player);
      }
      const fullscreen = document.fullscreenElement;
      const portal =
        fullscreen !== null &&
        (fullscreen === player || fullscreen.contains(player))
          ? fullscreen
          : document.documentElement;
      portal.append(overlay.host);
      overlay.reposition();
    }
  };

  function schedule(): void {
    if (destroyed || frame !== null) return;
    frame = window.requestAnimationFrame(() => {
      frame = null;
      refresh();
    });
  }

  const mutationObserver = new MutationObserverConstructor(() => schedule());
  mutationObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class", "style", "hidden"],
    childList: true,
    subtree: true,
  });
  window.addEventListener("resize", schedule);
  window.addEventListener("popstate", schedule);
  window.visualViewport?.addEventListener("resize", schedule);
  document.addEventListener("fullscreenchange", schedule);
  refresh();

  return Object.freeze({
    refresh,
    destroy(): void {
      if (destroyed) return;
      destroyed = true;
      if (frame !== null) window.cancelAnimationFrame(frame);
      frame = null;
      mutationObserver.disconnect();
      resizeObserver.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("popstate", schedule);
      window.visualViewport?.removeEventListener("resize", schedule);
      document.removeEventListener("fullscreenchange", schedule);
      removeAll();
    },
  });
}

function documentIdentity(value: string): string {
  const url = new URL(value);
  url.hash = "";
  return url.toString();
}
