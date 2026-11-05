import { EngineRejectedError } from "./engine-client.js";
import type { DownloadJob, JobAction } from "./protocol.js";

export type JobButtonAction = JobAction | "open" | "reveal" | "remove";

export interface JobButton {
  readonly action: JobButtonAction;
  readonly label: string;
  readonly destructive?: boolean;
}

export function availableJobButtons(job: DownloadJob): readonly JobButton[] {
  const buttons: JobButton[] = [];
  if (job.state === "queued" || job.state === "downloading") {
    buttons.push({ action: "pause", label: "Pause" });
  }
  if (job.state === "paused" || job.state === "failed") {
    buttons.push({ action: "resume", label: "Resume" });
  }
  if (
    ![
      "completed",
      "cancelled",
      "missing_local_file",
      "verifying",
      "assembling",
    ].includes(job.state)
  ) {
    buttons.push({ action: "cancel", label: "Cancel", destructive: true });
  }
  if (job.state === "completed") {
    buttons.push(
      { action: "open", label: "Open file" },
      { action: "reveal", label: "Open containing folder" },
    );
  }
  if (
    [
      "created",
      "paused",
      "completed",
      "needs_authentication",
      "needs_refresh",
      "blocked",
      "failed",
      "cancelled",
      "missing_local_file",
    ].includes(job.state)
  ) {
    buttons.push({ action: "remove", label: "Remove from list" });
  }
  return buttons;
}

export function confirmJobAction(
  job: DownloadJob,
  action: JobButtonAction,
  confirm: (message: string) => boolean,
): boolean {
  if (action === "cancel") {
    return confirm(
      `Cancel ${job.filename}? Any uncheckpointed transfer data may be lost.`,
    );
  }
  if (action === "remove") {
    return confirm(
      job.state === "completed"
        ? `Remove ${job.filename} from the list? The downloaded file will be kept.`
        : `Remove ${job.filename} from the list? Its saved partial data will be deleted.`,
    );
  }
  return true;
}

export function completedFilePath(job: DownloadJob): string {
  if (job.state !== "completed") {
    throw new Error("Only a completed download has a published file path.");
  }
  const destination = job.destination.trim().replace(/[\\/]+$/u, "");
  if (destination.length === 0)
    throw new Error("Download destination is empty.");
  return `${destination}\\${job.filename}`;
}

export function actionableError(error: unknown): string {
  const code = error instanceof EngineRejectedError ? error.code : "";
  const detail = error instanceof Error ? error.message : String(error);
  const signature = `${code} ${detail}`.toLocaleLowerCase();

  if (
    /disk (?:is |became )?full|not enough (disk )?space|no space left|os error 112/u.test(
      signature,
    )
  ) {
    return "Not enough disk space. Free space on the destination drive, then resume the download.";
  }
  if (
    /access denied|permission denied|destination_unavailable|read-only|os error 5/u.test(
      signature,
    )
  ) {
    return "Windows denied access to that location. Choose a folder you can write to and try again.";
  }
  if (
    /remote object identity changed|changed remote|resume_not_safe|validator.*changed/u.test(
      signature,
    )
  ) {
    return "The remote file changed, so appending would be unsafe. Remove this entry and start a new download.";
  }
  if (
    /network|timed? out|dns|connection|connect|probe_failed|http request|sending request/u.test(
      signature,
    )
  ) {
    return "The network request failed. Check your connection and the URL, then try again.";
  }
  return detail.length > 0
    ? detail
    : "The download action could not be completed.";
}
