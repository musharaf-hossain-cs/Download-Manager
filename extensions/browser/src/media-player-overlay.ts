export const MEDIA_PLAYER_OVERLAY_HOST_ATTRIBUTE =
  "data-download-manager-media-overlay";

export interface BasicMediaPlayerOverlay {
  readonly host: HTMLElement;
  reposition(): void;
  destroy(): void;
}

export interface BasicMediaPlayerOverlayOptions {
  readonly document: Document;
  readonly player: Element;
  readonly onDownload: () => Promise<void> | void;
}

/** Mounts one isolated download button without modifying the player element. */
export function mountBasicMediaPlayerOverlay(
  options: BasicMediaPlayerOverlayOptions,
): BasicMediaPlayerOverlay {
  const { document, player, onDownload } = options;
  const host = document.createElement("div");
  host.setAttribute(MEDIA_PLAYER_OVERLAY_HOST_ATTRIBUTE, "");
  host.style.position = "fixed";
  host.style.transform = "translateX(-100%)";
  host.style.zIndex = "2147483647";
  host.style.pointerEvents = "none";

  const shadow = host.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = `
    :host { all: initial; }
    button {
      all: initial;
      box-sizing: border-box;
      border: 1px solid rgba(255, 255, 255, 0.75);
      border-radius: 6px;
      background: rgba(24, 24, 27, 0.92);
      color: #fff;
      cursor: pointer;
      font: 600 13px/1.2 system-ui, sans-serif;
      padding: 8px 10px;
      pointer-events: auto;
    }
    button:focus-visible { outline: 2px solid #60a5fa; outline-offset: 2px; }
  `;
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = "Download media";
  button.setAttribute("aria-label", "Download media");
  const activate = (): void => {
    void onDownload();
  };
  button.addEventListener("click", activate);
  shadow.append(style, button);
  document.documentElement.append(host);

  let destroyed = false;
  const reposition = (): void => {
    if (destroyed) return;
    const rect = player.getBoundingClientRect();
    host.style.top = `${String(Math.max(8, rect.top + 8))}px`;
    host.style.left = `${String(Math.max(8, rect.right - 8))}px`;
    host.style.display =
      player.isConnected && rect.width > 0 && rect.height > 0
        ? "block"
        : "none";
  };
  reposition();
  return Object.freeze({
    host,
    reposition,
    destroy(): void {
      if (destroyed) return;
      destroyed = true;
      button.removeEventListener("click", activate);
      host.remove();
    },
  });
}
