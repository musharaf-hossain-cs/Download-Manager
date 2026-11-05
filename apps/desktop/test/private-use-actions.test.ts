import { describe, expect, it } from "vitest";

import { EngineRejectedError } from "../src/engine-client.js";
import {
  actionableError,
  availableJobButtons,
  completedFilePath,
  confirmJobAction,
} from "../src/private-use-actions.js";
import { jobFixture } from "./fixtures.js";

describe("private-use job actions", () => {
  it("offers only state-safe controls", () => {
    expect(
      availableJobButtons(jobFixture({ state: "downloading" })).map(
        ({ action }) => action,
      ),
    ).toEqual(["pause", "cancel"]);
    expect(
      availableJobButtons(jobFixture({ state: "paused" })).map(
        ({ action }) => action,
      ),
    ).toEqual(["resume", "cancel", "remove"]);
    expect(
      availableJobButtons(jobFixture({ state: "completed" })).map(
        ({ action }) => action,
      ),
    ).toEqual(["open", "reveal", "remove"]);
  });

  it("builds the exact completed path with Unicode and trailing separators", () => {
    const job = jobFixture({
      state: "completed",
      destination: "C:\\Downloads\\",
      filename: "বাংলা archive.zip",
    });
    expect(completedFilePath(job)).toBe("C:\\Downloads\\বাংলা archive.zip");
    expect(() => completedFilePath(jobFixture({ state: "paused" }))).toThrow(
      "completed",
    );
  });

  it("requires and honors confirmation only for destructive actions", () => {
    const prompts: string[] = [];
    const reject = (message: string) => {
      prompts.push(message);
      return false;
    };
    const active = jobFixture({ state: "downloading" });
    expect(confirmJobAction(active, "pause", reject)).toBe(true);
    expect(confirmJobAction(active, "cancel", reject)).toBe(false);
    expect(
      confirmJobAction(jobFixture({ state: "paused" }), "remove", reject),
    ).toBe(false);
    expect(prompts).toEqual([
      expect.stringContaining("uncheckpointed transfer data"),
      expect.stringContaining("partial data will be deleted"),
    ]);
  });

  it("turns common backend failures into recovery guidance", () => {
    expect(actionableError(new Error("disk became full at offset 4"))).toMatch(
      /Free space/u,
    );
    expect(actionableError(new Error("Access denied (os error 5)"))).toMatch(
      /Choose a folder/u,
    );
    expect(
      actionableError(
        new EngineRejectedError(
          "resume_not_safe",
          "remote object identity changed",
        ),
      ),
    ).toMatch(/start a new download/u);
    expect(actionableError(new Error("connection timed out"))).toMatch(
      /Check your connection/u,
    );
  });
});
