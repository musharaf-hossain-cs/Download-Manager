import {
  normalizeDropInputs,
  type DropInput,
  type DropNormalizationReport,
} from "./drag-drop.js";
import type { DownloadJob } from "./protocol.js";

export interface ScreenPoint {
  readonly x: number;
  readonly y: number;
}

export interface ScreenBounds extends ScreenPoint {
  readonly width: number;
  readonly height: number;
}

export interface DropTargetSnapshot {
  readonly enabled: boolean;
  readonly position: ScreenPoint;
}

export interface DesktopActionPort {
  saveDropTarget(snapshot: DropTargetSnapshot): Promise<void>;
  openFile(path: string): Promise<void>;
  openFolder(path: string): Promise<void>;
}

/** Owns the floating drop target preference and exact OS file/folder actions. */
export class DesktopActionController {
  #dropTarget: DropTargetSnapshot;

  public constructor(
    persisted: Partial<DropTargetSnapshot>,
    private readonly port: DesktopActionPort,
  ) {
    this.#dropTarget = normalizeSnapshot(persisted);
  }

  public get dropTarget(): DropTargetSnapshot {
    return cloneSnapshot(this.#dropTarget);
  }

  public async setDropTargetEnabled(enabled: boolean): Promise<void> {
    await this.#persist({ ...this.#dropTarget, enabled });
  }

  public async moveDropTarget(
    position: ScreenPoint,
    workArea: ScreenBounds,
  ): Promise<void> {
    assertFinitePoint(position);
    assertBounds(workArea);
    await this.#persist({
      ...this.#dropTarget,
      position: {
        x: clamp(position.x, workArea.x, workArea.x + workArea.width),
        y: clamp(position.y, workArea.y, workArea.y + workArea.height),
      },
    });
  }

  public acceptDrop(inputs: readonly DropInput[]): DropNormalizationReport {
    if (!this.#dropTarget.enabled) {
      throw new Error("floating drop target is disabled");
    }
    return normalizeDropInputs(inputs);
  }

  public async openJobFile(job: DownloadJob): Promise<void> {
    if (job.state !== "completed") {
      throw new Error("only completed downloads can be opened");
    }
    await this.port.openFile(jobPath(job));
  }

  public async openJobFolder(job: DownloadJob): Promise<void> {
    await this.port.openFolder(normalizeFolder(job.destination));
  }

  async #persist(next: DropTargetSnapshot): Promise<void> {
    const previous = this.#dropTarget;
    this.#dropTarget = next;
    try {
      await this.port.saveDropTarget(this.dropTarget);
    } catch (error) {
      this.#dropTarget = previous;
      throw error;
    }
  }
}

function normalizeSnapshot(
  persisted: Partial<DropTargetSnapshot>,
): DropTargetSnapshot {
  const position = persisted.position;
  return {
    enabled: persisted.enabled === true,
    position:
      position !== undefined && isFinitePoint(position)
        ? { x: position.x, y: position.y }
        : { x: 24, y: 24 },
  };
}

function jobPath(job: DownloadJob): string {
  const folder = normalizeFolder(job.destination);
  return `${folder}${folder.endsWith("\\") ? "" : "\\"}${job.filename}`;
}

function normalizeFolder(folder: string): string {
  const normalized = folder.trim();
  if (normalized.length === 0) throw new Error("job destination is empty");
  return /^[A-Za-z]:\\$/u.test(normalized)
    ? normalized
    : normalized.replace(/[\\/]+$/u, "");
}

function cloneSnapshot(snapshot: DropTargetSnapshot): DropTargetSnapshot {
  return { enabled: snapshot.enabled, position: { ...snapshot.position } };
}

function isFinitePoint(value: ScreenPoint): boolean {
  return Number.isFinite(value.x) && Number.isFinite(value.y);
}

function assertFinitePoint(value: ScreenPoint): void {
  if (!isFinitePoint(value)) throw new RangeError("position must be finite");
}

function assertBounds(bounds: ScreenBounds): void {
  assertFinitePoint(bounds);
  if (
    !Number.isFinite(bounds.width) ||
    !Number.isFinite(bounds.height) ||
    bounds.width < 0 ||
    bounds.height < 0
  ) {
    throw new RangeError("work area must have non-negative finite dimensions");
  }
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum);
}
