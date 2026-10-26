import { describe, expect, it } from "vitest";

import {
  DesktopAccessibilityModel,
  type DesktopAccessibilityInput,
} from "../src/accessibility.js";

const mainWindow = (): DesktopAccessibilityInput => ({
  windowName: "Download Manager",
  toolbarName: "Download controls",
  downloadsName: "Downloads",
  toolbar: [
    { id: "add", name: "Add URL" },
    { id: "resume", name: "Resume", disabled: true },
    { id: "stop", name: "Stop" },
    { id: "delete", name: "Delete" },
    { id: "options", name: "Options" },
  ],
  columns: [
    { id: "name", name: "Name", sort: "ascending" },
    { id: "status", name: "Status" },
    { id: "progress", name: "Progress" },
  ],
  rows: [
    {
      id: "job-a",
      name: "archive.zip",
      stateText: "Downloading",
      stateSymbol: "↓",
      cells: {
        name: "archive.zip",
        status: "Downloading",
        progress: "42 percent",
      },
    },
    {
      id: "job-b",
      name: "manual.pdf",
      stateText: "Completed",
      stateSymbol: "✓",
      cells: {
        name: "manual.pdf",
        status: "Completed",
        progress: "100 percent",
      },
    },
  ],
});

describe("desktop accessibility contract", () => {
  it("passes the automated semantic and high-contrast scan", () => {
    const model = new DesktopAccessibilityModel(mainWindow());

    expect(model.scan()).toEqual([]);
    expect(
      model.nodes
        .filter((node) => node.role === "columnheader")
        .map(({ name, columnIndex, sort }) => ({ name, columnIndex, sort })),
    ).toEqual([
      { name: "Name", columnIndex: 1, sort: "ascending" },
      { name: "Status", columnIndex: 2, sort: "none" },
      { name: "Progress", columnIndex: 3, sort: "none" },
    ]);
    expect(
      model.nodes
        .filter((node) => node.role === "row")
        .map(({ name, stateText, stateSymbol }) => ({
          name,
          stateText,
          stateSymbol,
        })),
    ).toEqual([
      {
        name: "archive.zip, Downloading",
        stateText: "Downloading",
        stateSymbol: "↓",
      },
      {
        name: "manual.pdf, Completed",
        stateText: "Completed",
        stateSymbol: "✓",
      },
    ]);
    expect(
      model.nodes.every(
        (node) =>
          node.style.foreground === "CanvasText" &&
          node.style.background === "Canvas" &&
          node.style.focusOutline === "Highlight",
      ),
    ).toBe(true);
  });

  it("performs a deterministic keyboard walkthrough with visible focus", () => {
    const model = new DesktopAccessibilityModel(mainWindow());
    const walkthrough = [
      model.key("Tab"),
      model.key("ArrowRight"),
      model.key("End"),
      model.key("Tab"),
      model.key("ArrowRight"),
      model.key("End"),
      model.key("Tab"),
      model.key("ArrowDown"),
      model.key("Enter"),
      model.key("Tab", true),
    ];

    expect(walkthrough).toEqual([
      { focusedId: "toolbar.add", activatedId: null },
      { focusedId: "toolbar.stop", activatedId: null },
      { focusedId: "toolbar.options", activatedId: null },
      { focusedId: "column.name", activatedId: null },
      { focusedId: "column.status", activatedId: null },
      { focusedId: "column.progress", activatedId: null },
      { focusedId: "download.job-a", activatedId: null },
      { focusedId: "download.job-b", activatedId: null },
      { focusedId: "download.job-b", activatedId: "download.job-b" },
      { focusedId: "download.job-a", activatedId: null },
    ]);
    expect(model.focused).toMatchObject({
      id: "download.job-a",
      role: "row",
      style: { focusOutline: "Highlight", focusOutlineWidth: 2 },
    });
  });

  it("reports missing labels and color-only download states", () => {
    const invalid = mainWindow();
    const model = new DesktopAccessibilityModel({
      ...invalid,
      toolbar: [{ id: "add", name: " " }],
      rows: [
        {
          ...invalid.rows[0]!,
          stateText: "",
          stateSymbol: "",
        },
      ],
    });

    expect(model.scan().map(({ nodeId, rule }) => `${nodeId}:${rule}`)).toEqual(
      expect.arrayContaining([
        "toolbar.add:accessible-name",
        "download.job-a:non-color-state",
      ]),
    );
  });
});
