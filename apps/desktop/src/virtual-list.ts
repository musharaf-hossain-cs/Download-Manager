import type { DownloadJob } from "./protocol.js";

export type JobSortColumn =
  | "filename"
  | "total_bytes"
  | "state"
  | "downloaded_bytes"
  | "created_at"
  | "completed_at"
  | "display_url"
  | "category"
  | "queue";
export type SortDirection = "ascending" | "descending";

export interface JobSort {
  readonly column: JobSortColumn;
  readonly direction: SortDirection;
}

export interface VirtualRow {
  readonly index: number;
  readonly offset: number;
  readonly job: DownloadJob;
}

export interface VirtualWindow {
  readonly totalHeight: number;
  readonly startIndex: number;
  readonly endIndex: number;
  readonly rows: readonly VirtualRow[];
}

const textCollator = new Intl.Collator(undefined, {
  numeric: true,
  sensitivity: "base",
});

function compareValues(
  left: string | number | null,
  right: string | number | null,
): number {
  if (left === right) return 0;
  if (left === null) return 1;
  if (right === null) return -1;
  return typeof left === "string" && typeof right === "string"
    ? textCollator.compare(left, right)
    : Number(left) - Number(right);
}

/** Stable non-mutating sort used by clickable list headers. */
export function sortJobs(
  jobs: readonly DownloadJob[],
  sort: JobSort,
): readonly DownloadJob[] {
  const direction = sort.direction === "ascending" ? 1 : -1;
  return jobs
    .map((job, index) => ({ job, index }))
    .sort((left, right) => {
      const compared = compareValues(
        left.job[sort.column],
        right.job[sort.column],
      );
      return compared === 0 ? left.index - right.index : compared * direction;
    })
    .map(({ job }) => job);
}

/** Fixed-row virtualization model; the renderer receives only the visible window. */
export class VirtualDownloadList {
  readonly #jobs: readonly DownloadJob[];

  public constructor(
    jobs: readonly DownloadJob[],
    sort: JobSort,
    private readonly rowHeight = 36,
    private readonly overscan = 4,
  ) {
    if (rowHeight <= 0 || overscan < 0)
      throw new RangeError("invalid virtual list sizing");
    this.#jobs = sortJobs(jobs, sort);
  }

  public window(scrollTop: number, viewportHeight: number): VirtualWindow {
    const safeTop = Math.max(0, scrollTop);
    const safeHeight = Math.max(0, viewportHeight);
    const visibleStart = Math.floor(safeTop / this.rowHeight);
    const visibleEnd = Math.ceil((safeTop + safeHeight) / this.rowHeight);
    const startIndex = Math.max(0, visibleStart - this.overscan);
    const endIndex = Math.min(this.#jobs.length, visibleEnd + this.overscan);
    const rows: VirtualRow[] = [];
    for (let index = startIndex; index < endIndex; index += 1) {
      const job = this.#jobs[index];
      if (job !== undefined)
        rows.push({ index, offset: index * this.rowHeight, job });
    }
    return {
      totalHeight: this.#jobs.length * this.rowHeight,
      startIndex,
      endIndex,
      rows,
    };
  }
}
