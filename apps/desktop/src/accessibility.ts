import type { DownloadColumnId } from "./appearance.js";

export type AccessibilityRole =
  | "application"
  | "toolbar"
  | "button"
  | "grid"
  | "columnheader"
  | "row"
  | "gridcell";

export type AccessibilityKey =
  | "Tab"
  | "ArrowLeft"
  | "ArrowRight"
  | "ArrowUp"
  | "ArrowDown"
  | "Home"
  | "End"
  | "Enter"
  | " ";

export type AccessibleGroup = "toolbar" | "columns" | "downloads";
export type AriaSort = "none" | "ascending" | "descending";

export interface AccessibleToolbarControl {
  readonly id: string;
  readonly name: string;
  readonly disabled?: boolean;
}

export interface AccessibleColumn {
  readonly id: DownloadColumnId;
  readonly name: string;
  readonly sort?: AriaSort;
}

export interface AccessibleDownloadRow {
  readonly id: string;
  readonly name: string;
  /** Localized state text announced by screen readers and shown beside the icon. */
  readonly stateText: string;
  /** A non-color status cue, such as an icon or short glyph. */
  readonly stateSymbol: string;
  readonly cells: Readonly<Partial<Record<DownloadColumnId, string>>>;
}

export interface DesktopAccessibilityInput {
  readonly windowName: string;
  readonly toolbarName: string;
  readonly downloadsName: string;
  readonly toolbar: readonly AccessibleToolbarControl[];
  readonly columns: readonly AccessibleColumn[];
  readonly rows: readonly AccessibleDownloadRow[];
}

export interface HighContrastStyle {
  readonly foreground: "CanvasText";
  readonly background: "Canvas";
  readonly border: "ButtonText";
  readonly focusOutline: "Highlight";
  readonly focusOutlineWidth: 2;
  readonly forcedColorAdjust: "auto";
}

export interface AccessibilityNode {
  readonly id: string;
  readonly role: AccessibilityRole;
  readonly name: string;
  readonly focusable: boolean;
  readonly disabled: boolean;
  readonly group: AccessibleGroup | null;
  readonly columnIndex: number | null;
  readonly rowIndex: number | null;
  readonly sort: AriaSort | null;
  readonly stateText: string | null;
  readonly stateSymbol: string | null;
  readonly style: HighContrastStyle;
}

export interface AccessibilityViolation {
  readonly nodeId: string;
  readonly rule:
    | "duplicate-id"
    | "accessible-name"
    | "focus-indicator"
    | "grid-semantics"
    | "non-color-state"
    | "high-contrast";
  readonly message: string;
}

export interface KeyboardResult {
  readonly focusedId: string | null;
  readonly activatedId: string | null;
}

export interface WindowsScaleProfile {
  readonly monitorId: string;
  readonly workAreaWidthPixels: number;
  readonly workAreaHeightPixels: number;
  /** Windows DPI scaling percentage. The supported release range is 100–250%. */
  readonly dpiPercent: number;
  /** Windows accessibility text scaling percentage. */
  readonly textPercent: number;
}

export interface ScaledDesktopLayout {
  readonly monitorId: string;
  readonly logicalWidth: number;
  readonly logicalHeight: number;
  readonly toolbarRows: number;
  readonly visibleToolbarControls: number;
  readonly overflowToolbarControls: number;
  readonly minimumControlWidth: number;
  readonly minimumControlHeight: number;
  readonly downloadsHeight: number;
}

const HIGH_CONTRAST_STYLE: HighContrastStyle = Object.freeze({
  foreground: "CanvasText",
  background: "Canvas",
  border: "ButtonText",
  focusOutline: "Highlight",
  focusOutlineWidth: 2,
  forcedColorAdjust: "auto",
});

/**
 * Renderer-neutral accessibility contract for the desktop main window.
 * Adapters map these semantics to native/HTML accessibility properties.
 */
export class DesktopAccessibilityModel {
  readonly #nodes: readonly AccessibilityNode[];
  #focusedId: string | null = null;

  public constructor(input: DesktopAccessibilityInput) {
    this.#nodes = buildNodes(input);
  }

  public get nodes(): readonly AccessibilityNode[] {
    return this.#nodes.map((node) => ({ ...node, style: { ...node.style } }));
  }

  public get focused(): AccessibilityNode | null {
    const node = this.#nodes.find(
      (candidate) => candidate.id === this.#focusedId,
    );
    return node === undefined ? null : { ...node, style: { ...node.style } };
  }

  /** Runs the deterministic semantic equivalent of the renderer's a11y scan. */
  public scan(): readonly AccessibilityViolation[] {
    const violations: AccessibilityViolation[] = [];
    const seen = new Set<string>();
    for (const node of this.#nodes) {
      if (seen.has(node.id)) {
        violations.push(violation(node, "duplicate-id", "ID must be unique"));
      }
      seen.add(node.id);
      if (node.name.trim().length === 0) {
        violations.push(
          violation(
            node,
            "accessible-name",
            "Role must have an accessible name",
          ),
        );
      }
      if (
        node.focusable &&
        (node.style.focusOutline !== "Highlight" ||
          node.style.focusOutlineWidth < 2)
      ) {
        violations.push(
          violation(
            node,
            "focus-indicator",
            "Focusable item needs a visible focus ring",
          ),
        );
      }
      if (!usesSystemColors(node.style)) {
        violations.push(
          violation(node, "high-contrast", "Use Windows forced-color tokens"),
        );
      }
      if (
        (node.role === "columnheader" && node.columnIndex === null) ||
        (node.role === "row" && node.rowIndex === null) ||
        (node.role === "gridcell" &&
          (node.rowIndex === null || node.columnIndex === null))
      ) {
        violations.push(
          violation(node, "grid-semantics", "Grid position must be exposed"),
        );
      }
      if (
        node.role === "row" &&
        ((node.stateText?.trim().length ?? 0) === 0 ||
          (node.stateSymbol?.trim().length ?? 0) === 0)
      ) {
        violations.push(
          violation(
            node,
            "non-color-state",
            "Download state needs text and a non-color symbol",
          ),
        );
      }
    }
    return violations;
  }

  /** Applies one keyboard event using toolbar/grid roving focus semantics. */
  public key(key: AccessibilityKey, shiftKey = false): KeyboardResult {
    const focusable = this.#nodes.filter(
      (node) => node.focusable && !node.disabled,
    );
    if (focusable.length === 0) return { focusedId: null, activatedId: null };
    const currentIndex = focusable.findIndex(
      (node) => node.id === this.#focusedId,
    );
    if (key === "Enter" || key === " ") {
      return {
        focusedId: this.#focusedId,
        activatedId:
          currentIndex < 0 ? null : (focusable[currentIndex]?.id ?? null),
      };
    }

    let targetIndex: number;
    if (key === "Tab") {
      const step = shiftKey ? -1 : 1;
      targetIndex = clamp(
        currentIndex < 0
          ? shiftKey
            ? focusable.length - 1
            : 0
          : currentIndex + step,
        0,
        focusable.length - 1,
      );
    } else {
      const current = focusable[currentIndex < 0 ? 0 : currentIndex];
      const group = current?.group;
      const groupIndexes = focusable
        .map((node, index) => ({ node, index }))
        .filter((entry) => entry.node.group === group)
        .map((entry) => entry.index);
      const groupPosition = Math.max(0, groupIndexes.indexOf(currentIndex));
      if (key === "Home") targetIndex = groupIndexes[0] ?? 0;
      else if (key === "End")
        targetIndex = groupIndexes.at(-1) ?? focusable.length - 1;
      else {
        const step = key === "ArrowLeft" || key === "ArrowUp" ? -1 : 1;
        targetIndex =
          groupIndexes[
            clamp(groupPosition + step, 0, groupIndexes.length - 1)
          ] ?? 0;
      }
    }
    this.#focusedId = focusable[targetIndex]?.id ?? null;
    return { focusedId: this.#focusedId, activatedId: null };
  }
}

/**
 * Plans the renderer's responsive toolbar and downloads region in logical pixels.
 * Controls wrap for at most two rows; remaining controls stay keyboard reachable
 * in the toolbar overflow instead of being clipped at high DPI or text scaling.
 */
export function planScaledDesktopLayout(
  profile: WindowsScaleProfile,
  toolbarNames: readonly string[],
): ScaledDesktopLayout {
  if (
    !Number.isFinite(profile.workAreaWidthPixels) ||
    !Number.isFinite(profile.workAreaHeightPixels) ||
    profile.workAreaWidthPixels <= 0 ||
    profile.workAreaHeightPixels <= 0
  ) {
    throw new RangeError("monitor work area must be positive");
  }
  if (
    !Number.isFinite(profile.dpiPercent) ||
    profile.dpiPercent < 100 ||
    profile.dpiPercent > 250
  ) {
    throw new RangeError("DPI scaling must be between 100 and 250 percent");
  }
  if (
    !Number.isFinite(profile.textPercent) ||
    profile.textPercent < 100 ||
    profile.textPercent > 225
  ) {
    throw new RangeError("text scaling must be between 100 and 225 percent");
  }
  if (toolbarNames.some((name) => name.trim().length === 0)) {
    throw new Error("toolbar controls need accessible names before layout");
  }

  const logicalWidth = Math.floor(
    (profile.workAreaWidthPixels * 100) / profile.dpiPercent,
  );
  const logicalHeight = Math.floor(
    (profile.workAreaHeightPixels * 100) / profile.dpiPercent,
  );
  const textScale = profile.textPercent / 100;
  const longestLabel = toolbarNames.reduce(
    (longest, name) =>
      Array.from(name).length > Array.from(longest).length ? name : longest,
    "",
  );
  const minimumControlWidth = Math.max(
    44,
    Math.ceil(Array.from(longestLabel).length * 8 * textScale + 32),
  );
  const minimumControlHeight = Math.max(44, Math.ceil(32 * textScale + 12));
  const columns = Math.max(1, Math.floor(logicalWidth / minimumControlWidth));
  const twoRowCapacity = columns * 2;
  const visibleToolbarControls = Math.min(toolbarNames.length, twoRowCapacity);
  const overflowToolbarControls = toolbarNames.length - visibleToolbarControls;
  const toolbarRows =
    visibleToolbarControls === 0
      ? 0
      : Math.min(2, Math.ceil(visibleToolbarControls / columns));
  const downloadsHeight = Math.max(
    0,
    logicalHeight - toolbarRows * minimumControlHeight - 64,
  );

  return {
    monitorId: profile.monitorId,
    logicalWidth,
    logicalHeight,
    toolbarRows,
    visibleToolbarControls,
    overflowToolbarControls,
    minimumControlWidth,
    minimumControlHeight,
    downloadsHeight,
  };
}

function buildNodes(
  input: DesktopAccessibilityInput,
): readonly AccessibilityNode[] {
  const nodes: AccessibilityNode[] = [
    node("window", "application", input.windowName, false, null),
    node("toolbar", "toolbar", input.toolbarName, false, null),
    ...input.toolbar.map((control) => ({
      ...node(`toolbar.${control.id}`, "button", control.name, true, "toolbar"),
      disabled: control.disabled === true,
    })),
    node("downloads", "grid", input.downloadsName, false, null),
  ];
  input.columns.forEach((column, columnIndex) => {
    nodes.push({
      ...node(
        `column.${column.id}`,
        "columnheader",
        column.name,
        true,
        "columns",
      ),
      columnIndex: columnIndex + 1,
      sort: column.sort ?? "none",
    });
  });
  input.rows.forEach((row, rowIndex) => {
    nodes.push({
      ...node(
        `download.${row.id}`,
        "row",
        `${row.name}, ${row.stateText}`,
        true,
        "downloads",
      ),
      rowIndex: rowIndex + 1,
      stateText: row.stateText,
      stateSymbol: row.stateSymbol,
    });
    input.columns.forEach((column, columnIndex) => {
      nodes.push({
        ...node(
          `download.${row.id}.${column.id}`,
          "gridcell",
          row.cells[column.id] ?? "",
          false,
          null,
        ),
        columnIndex: columnIndex + 1,
        rowIndex: rowIndex + 1,
      });
    });
  });
  return nodes;
}

function node(
  id: string,
  role: AccessibilityRole,
  name: string,
  focusable: boolean,
  group: AccessibleGroup | null,
): AccessibilityNode {
  return {
    id,
    role,
    name,
    focusable,
    disabled: false,
    group,
    columnIndex: null,
    rowIndex: null,
    sort: null,
    stateText: null,
    stateSymbol: null,
    style: HIGH_CONTRAST_STYLE,
  };
}

function violation(
  node: AccessibilityNode,
  rule: AccessibilityViolation["rule"],
  message: string,
): AccessibilityViolation {
  return { nodeId: node.id, rule, message };
}

function usesSystemColors(style: HighContrastStyle): boolean {
  return (
    style.foreground === "CanvasText" &&
    style.background === "Canvas" &&
    style.border === "ButtonText" &&
    style.forcedColorAdjust === "auto"
  );
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum);
}
