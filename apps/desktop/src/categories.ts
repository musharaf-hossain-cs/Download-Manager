export const DEFAULT_CATEGORY_NAMES = [
  "General",
  "Compressed",
  "Documents",
  "Music",
  "Programs",
  "Video",
] as const;

export type DefaultCategoryName = (typeof DEFAULT_CATEGORY_NAMES)[number];

export interface CategoryDestination {
  /** Folder used until a remembered folder is available. */
  readonly defaultFolder: string;
  readonly rememberLastFolder: boolean;
  readonly lastFolder: string | null;
}

export interface DownloadCategory {
  /** Stable identifier used by persisted jobs and later category rules. */
  readonly id: string;
  readonly name: string;
  readonly destination: CategoryDestination;
}

/**
 * Creates the deterministic first-run category set.
 *
 * General downloads use the selected downloads folder. Specialized categories
 * use named child folders and may remember a later per-category choice.
 */
export function createDefaultCategories(
  downloadsFolder: string,
): readonly DownloadCategory[] {
  const root = normalizeRoot(downloadsFolder);
  return DEFAULT_CATEGORY_NAMES.map((name) => ({
    id: name.toLocaleLowerCase("en-US"),
    name,
    destination: {
      defaultFolder: name === "General" ? root : joinWindowsPath(root, name),
      rememberLastFolder: true,
      lastFolder: null,
    },
  }));
}

/** Chooses a category's effective destination without mutating its defaults. */
export function resolveCategoryDestination(category: DownloadCategory): string {
  const { defaultFolder, rememberLastFolder, lastFolder } =
    category.destination;
  return rememberLastFolder && lastFolder !== null
    ? normalizeRoot(lastFolder)
    : normalizeRoot(defaultFolder);
}

function normalizeRoot(folder: string): string {
  const trimmed = folder.trim();
  if (trimmed.length === 0) {
    throw new Error("downloads folder must not be empty");
  }
  if (/^[A-Za-z]:\\$/.test(trimmed)) return trimmed;
  return trimmed.replace(/[\\/]+$/, "");
}

function joinWindowsPath(parent: string, child: string): string {
  return parent.endsWith("\\") ? `${parent}${child}` : `${parent}\\${child}`;
}
