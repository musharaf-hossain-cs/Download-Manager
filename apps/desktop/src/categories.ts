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

export type CategoryRuleKind = "extension" | "mime" | "host";

/** Rules are evaluated in array order; the first matching rule wins. */
export interface CategoryRule {
  readonly id: string;
  readonly categoryId: string;
  readonly kind: CategoryRuleKind;
  readonly value: string;
}

export interface CategoryMatchInput {
  readonly extension: string | null;
  readonly mimeType: string | null;
  readonly sourceHost: string | null;
}

export interface CategoryRuleMatch {
  readonly categoryId: string;
  readonly matchedRuleId: string | null;
  /** One-based position shown as the rule's first-match priority. */
  readonly priority: number | null;
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

/** Selects the first matching category rule, or the General category. */
export function matchCategoryRules(
  input: CategoryMatchInput,
  rules: readonly CategoryRule[],
): CategoryRuleMatch {
  const candidate = {
    extension: normalizeExtension(input.extension),
    mime: normalizeMime(input.mimeType),
    host: normalizeHost(input.sourceHost),
  };

  for (const [index, rule] of rules.entries()) {
    if (matchesRule(rule, candidate)) {
      return {
        categoryId: rule.categoryId,
        matchedRuleId: rule.id,
        priority: index + 1,
      };
    }
  }
  return { categoryId: "general", matchedRuleId: null, priority: null };
}

function matchesRule(
  rule: CategoryRule,
  candidate: {
    readonly extension: string | null;
    readonly mime: string | null;
    readonly host: string | null;
  },
): boolean {
  if (rule.kind === "extension") {
    const value = normalizeExtension(rule.value);
    return value !== null && value === candidate.extension;
  }
  if (rule.kind === "host") {
    const value = normalizeHost(rule.value);
    return value !== null && value === candidate.host;
  }
  const value = normalizeMime(rule.value);
  if (value === null || candidate.mime === null) return false;
  if (value.endsWith("/*")) {
    return candidate.mime.startsWith(value.slice(0, -1));
  }
  return value === candidate.mime;
}

function normalizeExtension(value: string | null): string | null {
  const normalized = value
    ?.trim()
    .replace(/^\.+/, "")
    .toLocaleLowerCase("en-US");
  return normalized === undefined || normalized.length === 0
    ? null
    : normalized;
}

function normalizeMime(value: string | null): string | null {
  const normalized = value?.split(";", 1)[0]?.trim().toLocaleLowerCase("en-US");
  return normalized === undefined || normalized.length === 0
    ? null
    : normalized;
}

function normalizeHost(value: string | null): string | null {
  const normalized = value
    ?.trim()
    .replace(/\.+$/, "")
    .toLocaleLowerCase("en-US");
  return normalized === undefined || normalized.length === 0
    ? null
    : normalized;
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
