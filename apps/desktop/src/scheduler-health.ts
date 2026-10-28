export interface ScheduledQueueRun {
  readonly queueName: string;
  /** RFC 3339 instant supplied by the scheduler. */
  readonly scheduledFor: string;
  /** IANA zone used when the schedule was evaluated. */
  readonly timeZone: string;
}

export type SchedulerSnapshot =
  | { readonly status: "disabled" }
  | { readonly status: "unavailable"; readonly reason: string }
  | {
      readonly status: "ready";
      readonly nextRun: ScheduledQueueRun | null;
    };

export interface SchedulerHealthPanel {
  readonly health: "disabled" | "warning" | "healthy";
  readonly title: string;
  readonly message: string;
  readonly nextRun: ScheduledQueueRun | null;
}

export interface SchedulerHealthPort {
  getSchedulerSnapshot(): Promise<SchedulerSnapshot>;
}

export type SchedulerHealthUiState =
  | { readonly status: "loading"; readonly panel: null }
  | { readonly status: "ready"; readonly panel: SchedulerHealthPanel };

/** Renderer-neutral next-run and scheduler-health model for the queue screen. */
export class SchedulerHealthController {
  #state: SchedulerHealthUiState = { status: "loading", panel: null };

  public constructor(private readonly port: SchedulerHealthPort) {}

  public get state(): SchedulerHealthUiState {
    return this.#state;
  }

  public async load(): Promise<void> {
    this.#state = { status: "loading", panel: null };
    try {
      this.#state = {
        status: "ready",
        panel: schedulerHealthPanel(await this.port.getSchedulerSnapshot()),
      };
    } catch (error) {
      this.#state = {
        status: "ready",
        panel: schedulerHealthPanel({
          status: "unavailable",
          reason:
            error instanceof Error
              ? error.message
              : "Scheduler status could not be read.",
        }),
      };
    }
  }
}

export function schedulerHealthPanel(
  snapshot: SchedulerSnapshot,
): SchedulerHealthPanel {
  if (snapshot.status === "disabled") {
    return {
      health: "disabled",
      title: "Scheduler disabled",
      message: "Enable scheduling to run queues automatically.",
      nextRun: null,
    };
  }
  if (snapshot.status === "unavailable") {
    return {
      health: "warning",
      title: "Scheduler unavailable",
      message: requiredText(snapshot.reason, "unavailable reason"),
      nextRun: null,
    };
  }
  if (snapshot.nextRun === null) {
    return {
      health: "healthy",
      title: "No scheduled queues",
      message: "The scheduler is running and has no upcoming queue runs.",
      nextRun: null,
    };
  }

  const nextRun = normalizeNextRun(snapshot.nextRun);
  return {
    health: "healthy",
    title: "Next queue run",
    message: `${nextRun.queueName} is scheduled for ${nextRun.scheduledFor} (${nextRun.timeZone}).`,
    nextRun,
  };
}

function normalizeNextRun(run: ScheduledQueueRun): ScheduledQueueRun {
  const scheduledFor = requiredText(run.scheduledFor, "scheduled time");
  if (
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(
      scheduledFor,
    ) ||
    Number.isNaN(Date.parse(scheduledFor))
  ) {
    throw new RangeError("scheduled time must be an RFC 3339 instant");
  }
  const timeZone = requiredText(run.timeZone, "time zone");
  try {
    new Intl.DateTimeFormat("en", { timeZone }).format(0);
  } catch {
    throw new RangeError("time zone must be a supported IANA zone");
  }
  return {
    queueName: requiredText(run.queueName, "queue name"),
    scheduledFor,
    timeZone,
  };
}

function requiredText(value: string, label: string): string {
  const trimmed = value.trim();
  if (trimmed.length === 0) throw new RangeError(`${label} must not be empty`);
  return trimmed;
}
