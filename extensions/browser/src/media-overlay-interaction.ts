export interface OverlayPoint {
  readonly x: number;
  readonly y: number;
}

export interface OverlayBounds extends OverlayPoint {
  readonly width: number;
  readonly height: number;
}

export interface MediaOverlaySiteState {
  readonly minimized: boolean;
  readonly position: OverlayPoint | null;
}

export interface MediaOverlayPreferencesSnapshot {
  readonly globalEnabled: boolean;
  readonly siteEnabled: Readonly<Record<string, boolean>>;
  readonly siteState: Readonly<Record<string, MediaOverlaySiteState>>;
}

export interface MediaOverlayPreferencesPort {
  save(snapshot: MediaOverlayPreferencesSnapshot): Promise<void>;
}

/** Owns persistent overlay enablement, minimization, and drag position. */
export class MediaOverlayInteractionController {
  #snapshot: MediaOverlayPreferencesSnapshot;

  public constructor(
    persisted: Partial<MediaOverlayPreferencesSnapshot>,
    private readonly port: MediaOverlayPreferencesPort,
  ) {
    this.#snapshot = normalizeSnapshot(persisted);
  }

  public get snapshot(): MediaOverlayPreferencesSnapshot {
    return cloneSnapshot(this.#snapshot);
  }

  public isEnabled(siteUrl: string): boolean {
    const site = siteHost(siteUrl);
    return this.#snapshot.siteEnabled[site] ?? this.#snapshot.globalEnabled;
  }

  public stateFor(siteUrl: string): MediaOverlaySiteState {
    const state = this.#snapshot.siteState[siteHost(siteUrl)];
    return state === undefined
      ? { minimized: false, position: null }
      : cloneSiteState(state);
  }

  public async setGlobalEnabled(enabled: boolean): Promise<void> {
    await this.#persist({ ...this.#snapshot, globalEnabled: enabled });
  }

  public async setSiteEnabled(
    siteUrl: string,
    enabled: boolean | null,
  ): Promise<void> {
    const site = siteHost(siteUrl);
    const siteEnabled = { ...this.#snapshot.siteEnabled };
    if (enabled === null) delete siteEnabled[site];
    else siteEnabled[site] = enabled;
    await this.#persist({ ...this.#snapshot, siteEnabled });
  }

  public async setMinimized(
    siteUrl: string,
    minimized: boolean,
  ): Promise<void> {
    const site = siteHost(siteUrl);
    const current = this.stateFor(siteUrl);
    await this.#persist({
      ...this.#snapshot,
      siteState: {
        ...this.#snapshot.siteState,
        [site]: { ...current, minimized },
      },
    });
  }

  public async move(
    siteUrl: string,
    position: OverlayPoint,
    bounds: OverlayBounds,
  ): Promise<void> {
    assertPoint(position);
    assertBounds(bounds);
    const site = siteHost(siteUrl);
    const current = this.stateFor(siteUrl);
    await this.#persist({
      ...this.#snapshot,
      siteState: {
        ...this.#snapshot.siteState,
        [site]: {
          ...current,
          position: {
            x: clamp(position.x, bounds.x, bounds.x + bounds.width),
            y: clamp(position.y, bounds.y, bounds.y + bounds.height),
          },
        },
      },
    });
  }

  async #persist(next: MediaOverlayPreferencesSnapshot): Promise<void> {
    const previous = this.#snapshot;
    this.#snapshot = normalizeSnapshot(next);
    try {
      await this.port.save(this.snapshot);
    } catch (error) {
      this.#snapshot = previous;
      throw error;
    }
  }
}

function normalizeSnapshot(
  persisted: Partial<MediaOverlayPreferencesSnapshot>,
): MediaOverlayPreferencesSnapshot {
  const siteEnabled: Record<string, boolean> = {};
  for (const [site, enabled] of Object.entries(persisted.siteEnabled ?? {})) {
    if (validHost(site) && typeof enabled === "boolean") {
      siteEnabled[site.toLowerCase()] = enabled;
    }
  }
  const siteState: Record<string, MediaOverlaySiteState> = {};
  for (const [site, state] of Object.entries(persisted.siteState ?? {})) {
    if (!validHost(site) || state === null || typeof state !== "object") {
      continue;
    }
    const position = state.position;
    siteState[site.toLowerCase()] = {
      minimized: state.minimized === true,
      position:
        position !== null && isPoint(position)
          ? { x: position.x, y: position.y }
          : null,
    };
  }
  return {
    globalEnabled: persisted.globalEnabled !== false,
    siteEnabled,
    siteState,
  };
}

function siteHost(siteUrl: string): string {
  const url = new URL(siteUrl);
  if (
    (url.protocol !== "http:" && url.protocol !== "https:") ||
    !validHost(url.hostname)
  ) {
    throw new TypeError("site URL must be an absolute HTTP(S) URL");
  }
  return url.hostname.toLowerCase();
}

function validHost(value: string): boolean {
  return (
    value.length > 0 &&
    value.length <= 253 &&
    /^[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$/iu.test(value) &&
    !value.includes("..")
  );
}

function cloneSnapshot(
  snapshot: MediaOverlayPreferencesSnapshot,
): MediaOverlayPreferencesSnapshot {
  return {
    globalEnabled: snapshot.globalEnabled,
    siteEnabled: { ...snapshot.siteEnabled },
    siteState: Object.fromEntries(
      Object.entries(snapshot.siteState).map(([site, state]) => [
        site,
        cloneSiteState(state),
      ]),
    ),
  };
}

function cloneSiteState(state: MediaOverlaySiteState): MediaOverlaySiteState {
  return {
    minimized: state.minimized,
    position: state.position === null ? null : { ...state.position },
  };
}

function isPoint(value: OverlayPoint): boolean {
  return Number.isFinite(value.x) && Number.isFinite(value.y);
}

function assertPoint(value: OverlayPoint): void {
  if (!isPoint(value)) throw new RangeError("position must be finite");
}

function assertBounds(bounds: OverlayBounds): void {
  assertPoint(bounds);
  if (
    !Number.isFinite(bounds.width) ||
    !Number.isFinite(bounds.height) ||
    bounds.width < 0 ||
    bounds.height < 0
  ) {
    throw new RangeError("bounds must have non-negative finite dimensions");
  }
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum);
}
