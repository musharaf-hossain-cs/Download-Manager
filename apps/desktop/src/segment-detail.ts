export interface SegmentTelemetry {
  readonly id: string;
  /** Inclusive byte range assigned to this connection. */
  readonly start: number;
  readonly end: number;
  /** First byte in the range that has not been durably written. */
  readonly durable_offset: number;
  readonly worker_state: string;
}

/** One engine-produced snapshot consumed by the renderer-facing detail model. */
export interface SegmentDetailSnapshot {
  readonly job_id: string;
  readonly at: number;
  readonly downloaded_bytes: number;
  readonly total_bytes: number | null;
  readonly segments: readonly SegmentTelemetry[];
}

export interface SegmentDetailRow {
  readonly id: string;
  readonly range_start: number;
  readonly range_end: number;
  readonly downloaded_bytes: number;
  readonly size_bytes: number;
  readonly progress_percent: number;
  readonly speed_bytes_per_second: number | null;
  readonly worker_state: string;
}

export interface SegmentDetailView {
  readonly job_id: string;
  readonly downloaded_bytes: number;
  readonly total_bytes: number | null;
  readonly progress_percent: number | null;
  readonly speed_bytes_per_second: number | null;
  /** Null means the ETA cannot yet be estimated; zero means complete. */
  readonly eta_seconds: number | null;
  readonly segments: readonly SegmentDetailRow[];
}

interface PreviousSample {
  readonly at: number;
  readonly downloadedBytes: number;
  readonly segmentOffsets: ReadonlyMap<string, number>;
}

/** Derives stable display values from ordered segment telemetry snapshots. */
export class SegmentDetailController {
  readonly #previous = new Map<string, PreviousSample>();
  #view: SegmentDetailView | null = null;

  public get view(): SegmentDetailView | null {
    return this.#view;
  }

  public apply(snapshot: SegmentDetailSnapshot): SegmentDetailView {
    validateSnapshot(snapshot);
    const previous = this.#previous.get(snapshot.job_id);
    const elapsedSeconds =
      previous === undefined || snapshot.at <= previous.at
        ? null
        : (snapshot.at - previous.at) / 1_000;
    const speed = rate(
      previous?.downloadedBytes,
      snapshot.downloaded_bytes,
      elapsedSeconds,
    );
    const segments = snapshot.segments.map((segment) => {
      const size = segment.end - segment.start + 1;
      const downloaded = segment.durable_offset - segment.start;
      return {
        id: segment.id,
        range_start: segment.start,
        range_end: segment.end,
        downloaded_bytes: downloaded,
        size_bytes: size,
        progress_percent: percent(downloaded, size),
        speed_bytes_per_second: rate(
          previous?.segmentOffsets.get(segment.id),
          segment.durable_offset,
          elapsedSeconds,
        ),
        worker_state: segment.worker_state,
      };
    });
    const view: SegmentDetailView = {
      job_id: snapshot.job_id,
      downloaded_bytes: snapshot.downloaded_bytes,
      total_bytes: snapshot.total_bytes,
      progress_percent:
        snapshot.total_bytes === null
          ? null
          : percent(snapshot.downloaded_bytes, snapshot.total_bytes),
      speed_bytes_per_second: speed,
      eta_seconds: eta(snapshot.downloaded_bytes, snapshot.total_bytes, speed),
      segments,
    };
    this.#previous.set(snapshot.job_id, {
      at: snapshot.at,
      downloadedBytes: snapshot.downloaded_bytes,
      segmentOffsets: new Map(
        snapshot.segments.map((segment) => [
          segment.id,
          segment.durable_offset,
        ]),
      ),
    });
    this.#view = view;
    return view;
  }
}

function rate(
  previous: number | undefined,
  current: number,
  elapsedSeconds: number | null,
): number | null {
  if (previous === undefined || elapsedSeconds === null || current < previous) {
    return null;
  }
  return Math.round((current - previous) / elapsedSeconds);
}

function eta(
  downloaded: number,
  total: number | null,
  speed: number | null,
): number | null {
  if (total === null) return null;
  const remaining = Math.max(0, total - downloaded);
  if (remaining === 0) return 0;
  return speed === null || speed <= 0 ? null : Math.ceil(remaining / speed);
}

function percent(downloaded: number, total: number): number {
  if (total === 0) return 100;
  return Math.min(100, Math.round((downloaded / total) * 10_000) / 100);
}

function validateSnapshot(snapshot: SegmentDetailSnapshot): void {
  if (
    !Number.isSafeInteger(snapshot.at) ||
    snapshot.at < 0 ||
    !Number.isSafeInteger(snapshot.downloaded_bytes) ||
    snapshot.downloaded_bytes < 0 ||
    (snapshot.total_bytes !== null &&
      (!Number.isSafeInteger(snapshot.total_bytes) ||
        snapshot.total_bytes < snapshot.downloaded_bytes))
  ) {
    throw new RangeError("invalid job progress snapshot");
  }
  for (const segment of snapshot.segments) {
    if (
      !Number.isSafeInteger(segment.start) ||
      !Number.isSafeInteger(segment.end) ||
      !Number.isSafeInteger(segment.durable_offset) ||
      segment.start < 0 ||
      segment.end < segment.start ||
      segment.durable_offset < segment.start ||
      segment.durable_offset > segment.end + 1
    ) {
      throw new RangeError(`invalid segment range for ${segment.id}`);
    }
  }
}
