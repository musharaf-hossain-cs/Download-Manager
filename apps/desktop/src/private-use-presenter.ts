import type { DownloadJob } from "./protocol.js";
import type { SegmentDetailView } from "./segment-detail.js";

export interface JobPresentation {
  readonly status: string;
  readonly progressLabel: string;
  readonly progressPercent: number | null;
  readonly transferLabel: string;
  readonly speedLabel: string;
  readonly etaLabel: string;
}

export function presentJob(
  job: DownloadJob,
  detail: SegmentDetailView,
): JobPresentation {
  const total = job.total_bytes;
  return {
    status: job.state
      .split("_")
      .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
      .join(" "),
    progressLabel:
      detail.progress_percent === null
        ? "Progress unknown"
        : `${formatPercent(detail.progress_percent)} complete`,
    progressPercent: detail.progress_percent,
    transferLabel:
      total === null
        ? `${formatBytes(job.downloaded_bytes)} transferred`
        : `${formatBytes(job.downloaded_bytes)} of ${formatBytes(total)}`,
    speedLabel:
      detail.speed_bytes_per_second === null
        ? "—"
        : `${formatBytes(detail.speed_bytes_per_second)}/s`,
    etaLabel: formatEta(detail.eta_seconds),
  };
}

export function formatBytes(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"] as const;
  let value = bytes;
  let unit: (typeof units)[number] = units[0];
  for (const candidate of units) {
    value /= 1_024;
    unit = candidate;
    if (value < 1_024 || candidate === units.at(-1)) break;
  }
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${unit}`;
}

export function formatEta(seconds: number | null): string {
  if (seconds === null) return "—";
  if (seconds <= 0) return "Done";
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

function formatPercent(percent: number): string {
  return Number.isInteger(percent) ? `${percent}%` : `${percent.toFixed(2)}%`;
}
