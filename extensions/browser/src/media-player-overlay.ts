import type { MediaOverlaySiteState } from "./media-overlay-interaction.js";

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
  readonly initialInteractionState?: MediaOverlaySiteState;
  readonly onInteractionChange?: (state: MediaOverlaySiteState) => void;
}

/** Mounts one isolated download button without modifying the player element. */
export function mountBasicMediaPlayerOverlay(
  options: BasicMediaPlayerOverlayOptions,
): BasicMediaPlayerOverlay {
  const { document, player, onDownload } = options;
  let interactionState = cloneInteractionState(
    options.initialInteractionState ?? { minimized: false, position: null },
  );
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
    button[hidden] { display: none; }
    .control { margin-inline-start: 4px; padding: 8px; }
    .drag { cursor: grab; touch-action: none; }
    button:focus-visible { outline: 2px solid #60a5fa; outline-offset: 2px; }
  `;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "download";
  button.textContent = "Download media";
  button.setAttribute("aria-label", "Download media");
  const activate = (): void => {
    void onDownload();
  };
  button.addEventListener("click", activate);
  const minimize = document.createElement("button");
  minimize.type = "button";
  minimize.className = "control minimize";
  minimize.setAttribute("aria-label", "Minimize media download panel");
  const dragHandle = document.createElement("button");
  dragHandle.type = "button";
  dragHandle.className = "control drag";
  dragHandle.textContent = "↕";
  dragHandle.setAttribute("aria-label", "Move media download panel");
  shadow.append(style, button, minimize, dragHandle);
  document.documentElement.append(host);

  let destroyed = false;
  let drag: {
    readonly pointerId: number;
    readonly start: { readonly x: number; readonly y: number };
    readonly origin: { readonly x: number; readonly y: number };
  } | null = null;
  const applyInteraction = (): void => {
    button.hidden = interactionState.minimized;
    minimize.textContent = interactionState.minimized ? "+" : "−";
    minimize.setAttribute(
      "aria-label",
      interactionState.minimized
        ? "Expand media download panel"
        : "Minimize media download panel",
    );
  };
  const changeInteraction = (state: MediaOverlaySiteState): void => {
    interactionState = cloneInteractionState(state);
    applyInteraction();
    options.onInteractionChange?.(cloneInteractionState(interactionState));
  };
  const reposition = (): void => {
    if (destroyed) return;
    const rect = player.getBoundingClientRect();
    if (interactionState.position === null) {
      host.style.top = `${String(Math.max(8, rect.top + 8))}px`;
      host.style.left = `${String(Math.max(8, rect.right - 8))}px`;
      host.style.transform = "translateX(-100%)";
    } else {
      host.style.top = `${String(interactionState.position.y)}px`;
      host.style.left = `${String(interactionState.position.x)}px`;
      host.style.transform = "none";
    }
    host.style.display =
      player.isConnected && rect.width > 0 && rect.height > 0
        ? "block"
        : "none";
  };
  const stopDragging = (event?: PointerEvent): void => {
    if (
      drag === null ||
      (event !== undefined && event.pointerId !== drag.pointerId)
    ) {
      return;
    }
    drag = null;
    document.removeEventListener("pointermove", moveDrag);
    document.removeEventListener("pointerup", stopDragging);
    options.onInteractionChange?.(cloneInteractionState(interactionState));
  };
  const moveDrag = (event: PointerEvent): void => {
    if (drag === null || event.pointerId !== drag.pointerId) return;
    interactionState = {
      ...interactionState,
      position: {
        x: drag.origin.x + event.clientX - drag.start.x,
        y: drag.origin.y + event.clientY - drag.start.y,
      },
    };
    reposition();
  };
  const startDragging = (event: PointerEvent): void => {
    if (event.button !== 0 || drag !== null) return;
    event.preventDefault();
    const rect = host.getBoundingClientRect();
    drag = {
      pointerId: event.pointerId,
      start: { x: event.clientX, y: event.clientY },
      origin: { x: rect.left, y: rect.top },
    };
    document.addEventListener("pointermove", moveDrag);
    document.addEventListener("pointerup", stopDragging);
  };
  minimize.addEventListener("click", () => {
    changeInteraction({
      ...interactionState,
      minimized: !interactionState.minimized,
    });
  });
  dragHandle.addEventListener("pointerdown", startDragging);
  applyInteraction();
  reposition();
  return Object.freeze({
    host,
    reposition,
    destroy(): void {
      if (destroyed) return;
      destroyed = true;
      stopDragging();
      button.removeEventListener("click", activate);
      host.remove();
    },
  });
}

function cloneInteractionState(
  state: MediaOverlaySiteState,
): MediaOverlaySiteState {
  return {
    minimized: state.minimized,
    position: state.position === null ? null : { ...state.position },
  };
}
