export type DownloadNotification =
  | {
      readonly type: "completed";
      readonly jobId: string;
      readonly filename: string;
      readonly path: string;
    }
  | {
      readonly type: "failed";
      readonly jobId: string;
      readonly filename: string;
      readonly reason: string;
    };

export interface WindowsToast {
  readonly title: string;
  readonly body: string;
  /** Replaces an older toast for the same download instead of producing noise. */
  readonly tag: string;
  readonly group: "downloads";
  readonly severity: "success" | "error" | "info";
}

export interface WindowsNotificationPort {
  isSupported(): Promise<boolean>;
  show(toast: WindowsToast): Promise<void>;
}

export type NotificationResult = "shown" | "disabled" | "unsupported";

/** Maps durable job outcomes to Windows notifications through the Tauri adapter. */
export class WindowsNotificationController {
  public constructor(
    private readonly port: WindowsNotificationPort,
    private readonly enabled: () => boolean,
  ) {}

  public async notify(
    event: DownloadNotification,
  ): Promise<NotificationResult> {
    if (!this.enabled()) return "disabled";
    if (!(await this.port.isSupported())) return "unsupported";
    await this.port.show(toToast(event));
    return "shown";
  }

  /** Emits an explicit diagnostic toast through the configured real adapter. */
  public async smoke(): Promise<NotificationResult> {
    if (!this.enabled()) return "disabled";
    if (!(await this.port.isSupported())) return "unsupported";
    await this.port.show({
      title: "Download Manager notifications are ready",
      body: "Completion and failure notifications can appear here.",
      tag: "download-manager:notification-smoke",
      group: "downloads",
      severity: "info",
    });
    return "shown";
  }
}

function toToast(event: DownloadNotification): WindowsToast {
  const filename = requiredText(event.filename, "filename");
  const jobId = requiredText(event.jobId, "job id");
  return event.type === "completed"
    ? {
        title: "Download complete",
        body: `${filename} was saved to ${requiredText(event.path, "path")}`,
        tag: `download:${jobId}`,
        group: "downloads",
        severity: "success",
      }
    : {
        title: "Download failed",
        body: `${filename}: ${requiredText(event.reason, "failure reason")}`,
        tag: `download:${jobId}`,
        group: "downloads",
        severity: "error",
      };
}

function requiredText(value: string, label: string): string {
  const trimmed = value.trim();
  if (trimmed.length === 0) throw new RangeError(`${label} must not be empty`);
  return trimmed;
}
