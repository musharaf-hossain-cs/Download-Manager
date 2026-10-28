import { describe, expect, it, vi } from "vitest";

import {
  SchedulerHealthController,
  schedulerHealthPanel,
  type SchedulerHealthPort,
  type SchedulerSnapshot,
} from "../src/scheduler-health.js";

describe("scheduler next-run and health UI", () => {
  it("shows a neutral disabled snapshot without an availability warning", () => {
    expect(schedulerHealthPanel({ status: "disabled" })).toEqual({
      health: "disabled",
      title: "Scheduler disabled",
      message: "Enable scheduling to run queues automatically.",
      nextRun: null,
    });
  });

  it("shows an actionable warning when scheduler health is unavailable", () => {
    expect(
      schedulerHealthPanel({
        status: "unavailable",
        reason: " Scheduler service is not running. ",
      }),
    ).toEqual({
      health: "warning",
      title: "Scheduler unavailable",
      message: "Scheduler service is not running.",
      nextRun: null,
    });
  });

  it("preserves the authoritative next-run instant and IANA zone", () => {
    expect(
      schedulerHealthPanel({
        status: "ready",
        nextRun: {
          queueName: " Nightly ",
          scheduledFor: "2026-11-01T17:30:00Z",
          timeZone: "Asia/Dhaka",
        },
      }),
    ).toEqual({
      health: "healthy",
      title: "Next queue run",
      message: "Nightly is scheduled for 2026-11-01T17:30:00Z (Asia/Dhaka).",
      nextRun: {
        queueName: "Nightly",
        scheduledFor: "2026-11-01T17:30:00Z",
        timeZone: "Asia/Dhaka",
      },
    });
  });

  it("distinguishes a healthy scheduler with no upcoming queue", () => {
    expect(
      schedulerHealthPanel({ status: "ready", nextRun: null }),
    ).toMatchObject({
      health: "healthy",
      title: "No scheduled queues",
      nextRun: null,
    });
  });

  it("turns a failed health request into an unavailable snapshot", async () => {
    const port: SchedulerHealthPort = {
      getSchedulerSnapshot: vi.fn(async () => {
        throw new Error("engine connection lost");
      }),
    };
    const controller = new SchedulerHealthController(port);

    await controller.load();

    expect(controller.state).toEqual({
      status: "ready",
      panel: {
        health: "warning",
        title: "Scheduler unavailable",
        message: "engine connection lost",
        nextRun: null,
      },
    });
  });

  it.each<SchedulerSnapshot>([
    { status: "unavailable", reason: " " },
    {
      status: "ready",
      nextRun: {
        queueName: "Nightly",
        scheduledFor: "2026-11-01",
        timeZone: "Asia/Dhaka",
      },
    },
    {
      status: "ready",
      nextRun: {
        queueName: "Nightly",
        scheduledFor: "2026-11-01T17:30Z",
        timeZone: "Asia/Dhaka",
      },
    },
    {
      status: "ready",
      nextRun: {
        queueName: "Nightly",
        scheduledFor: "2026-11-01T17:30:00Z",
        timeZone: "Invalid/Zone",
      },
    },
  ])("rejects malformed engine snapshot %#", (snapshot) => {
    expect(() => schedulerHealthPanel(snapshot)).toThrow();
  });
});
