export const DOWNLOAD_COLUMN_IDS = [
  "name",
  "size",
  "status",
  "progress",
  "speed",
  "eta",
  "transferred",
  "date_added",
  "date_completed",
  "url_host",
  "category",
  "queue",
  "description",
] as const;

export type DownloadColumnId = (typeof DOWNLOAD_COLUMN_IDS)[number];
export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = Exclude<ThemePreference, "system">;

export interface ColumnPreference {
  readonly id: DownloadColumnId;
  readonly visible: boolean;
}

export interface AppearanceSnapshot {
  readonly theme: ThemePreference;
  readonly columns: readonly ColumnPreference[];
}

export interface AppearanceStore {
  save(snapshot: AppearanceSnapshot): Promise<void>;
}

const DEFAULT_SNAPSHOT: AppearanceSnapshot = {
  theme: "system",
  columns: DOWNLOAD_COLUMN_IDS.map((id) => ({ id, visible: true })),
};

/** Owns renderer appearance preferences and persists every accepted change. */
export class AppearanceController {
  #snapshot: AppearanceSnapshot;

  public constructor(
    persisted: Partial<AppearanceSnapshot>,
    private readonly store: AppearanceStore,
  ) {
    this.#snapshot = normalizeSnapshot(persisted);
  }

  public get snapshot(): AppearanceSnapshot {
    return cloneSnapshot(this.#snapshot);
  }

  public resolveTheme(systemIsDark: boolean): ResolvedTheme {
    return this.#snapshot.theme === "system"
      ? systemIsDark
        ? "dark"
        : "light"
      : this.#snapshot.theme;
  }

  public async setTheme(theme: ThemePreference): Promise<void> {
    if (!isTheme(theme))
      throw new RangeError(`unknown theme: ${String(theme)}`);
    await this.#persist({ ...this.#snapshot, theme });
  }

  public async setColumnVisible(
    id: DownloadColumnId,
    visible: boolean,
  ): Promise<void> {
    this.#assertColumn(id);
    await this.#persist({
      ...this.#snapshot,
      columns: this.#snapshot.columns.map((column) =>
        column.id === id ? { ...column, visible } : column,
      ),
    });
  }

  /** Moves a column before another column, or to the end when beforeId is null. */
  public async moveColumn(
    id: DownloadColumnId,
    beforeId: DownloadColumnId | null,
  ): Promise<void> {
    this.#assertColumn(id);
    if (beforeId !== null) this.#assertColumn(beforeId);
    if (id === beforeId) return;
    const columns = this.#snapshot.columns.filter((column) => column.id !== id);
    const moved = this.#snapshot.columns.find((column) => column.id === id);
    if (moved === undefined) return;
    const target =
      beforeId === null
        ? columns.length
        : columns.findIndex((column) => column.id === beforeId);
    columns.splice(target, 0, moved);
    await this.#persist({ ...this.#snapshot, columns });
  }

  #assertColumn(id: DownloadColumnId): void {
    if (!DOWNLOAD_COLUMN_IDS.includes(id))
      throw new RangeError(`unknown column: ${String(id)}`);
  }

  async #persist(next: AppearanceSnapshot): Promise<void> {
    const previous = this.#snapshot;
    this.#snapshot = next;
    try {
      await this.store.save(this.snapshot);
    } catch (error) {
      this.#snapshot = previous;
      throw error;
    }
  }
}

function normalizeSnapshot(
  persisted: Partial<AppearanceSnapshot>,
): AppearanceSnapshot {
  const theme = isTheme(persisted.theme)
    ? persisted.theme
    : DEFAULT_SNAPSHOT.theme;
  const known = new Map<DownloadColumnId, boolean>();
  for (const column of persisted.columns ?? []) {
    if (
      DOWNLOAD_COLUMN_IDS.includes(column.id) &&
      !known.has(column.id) &&
      typeof column.visible === "boolean"
    ) {
      known.set(column.id, column.visible);
    }
  }
  const ordered = [...known].map(([id, visible]) => ({ id, visible }));
  for (const id of DOWNLOAD_COLUMN_IDS) {
    if (!known.has(id)) ordered.push({ id, visible: true });
  }
  return { theme, columns: ordered };
}

function isTheme(value: unknown): value is ThemePreference {
  return value === "light" || value === "dark" || value === "system";
}

function cloneSnapshot(snapshot: AppearanceSnapshot): AppearanceSnapshot {
  return {
    theme: snapshot.theme,
    columns: snapshot.columns.map((column) => ({ ...column })),
  };
}
