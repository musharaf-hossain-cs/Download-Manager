import type { DownloadJob } from "./protocol.js";

export interface HistoryFilter {
  readonly query: string;
  readonly category: string | null;
  readonly completedAfter: number | null;
}

export interface HistoryRecordStore {
  /** Deletes database history records only; completed files are outside this port. */
  clearRecords(jobIds: readonly string[]): Promise<void>;
}

export interface HistoryState {
  readonly records: readonly DownloadJob[];
  readonly visible: readonly DownloadJob[];
  readonly filter: HistoryFilter;
  readonly retentionDays: number | null;
}

const EMPTY_FILTER: HistoryFilter = {
  query: "",
  category: null,
  completedAfter: null,
};

/** Renderer-facing completed-download history, isolated from file deletion. */
export class HistoryController {
  #records: readonly DownloadJob[];
  #filter: HistoryFilter = EMPTY_FILTER;
  #retentionDays: number | null = null;

  public constructor(
    records: readonly DownloadJob[],
    private readonly store: HistoryRecordStore,
  ) {
    this.#records = records.filter((record) => record.state === "completed");
  }

  public get state(): HistoryState {
    return {
      records: this.#records,
      visible: this.#records.filter((record) => matches(record, this.#filter)),
      filter: this.#filter,
      retentionDays: this.#retentionDays,
    };
  }

  public setFilter(filter: Partial<HistoryFilter>): void {
    this.#filter = { ...this.#filter, ...filter };
  }

  public setRetentionDays(days: number | null): void {
    if (days !== null && (!Number.isInteger(days) || days < 1)) {
      throw new RangeError("retention must be a positive whole number of days");
    }
    this.#retentionDays = days;
  }

  public async clearVisible(): Promise<readonly string[]> {
    return this.#clear(this.state.visible.map((record) => record.id));
  }

  public async applyRetention(now: number): Promise<readonly string[]> {
    if (this.#retentionDays === null) return [];
    const cutoff = now - this.#retentionDays * 86_400_000;
    return this.#clear(
      this.#records
        .filter(
          (record) =>
            record.completed_at !== null && record.completed_at < cutoff,
        )
        .map((record) => record.id),
    );
  }

  async #clear(jobIds: readonly string[]): Promise<readonly string[]> {
    if (jobIds.length === 0) return [];
    await this.store.clearRecords(jobIds);
    const removed = new Set(jobIds);
    this.#records = this.#records.filter((record) => !removed.has(record.id));
    return jobIds;
  }
}

function matches(record: DownloadJob, filter: HistoryFilter): boolean {
  if (filter.category !== null && record.category !== filter.category)
    return false;
  if (
    filter.completedAfter !== null &&
    (record.completed_at === null ||
      record.completed_at < filter.completedAfter)
  )
    return false;
  const query = filter.query.trim().toLocaleLowerCase();
  if (query.length === 0) return true;
  return [
    record.filename,
    record.display_url,
    record.destination,
    record.category ?? "",
  ]
    .join("\n")
    .toLocaleLowerCase()
    .includes(query);
}
