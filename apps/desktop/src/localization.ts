export const MESSAGE_KEYS = [
  "app.title",
  "toolbar.addUrl",
  "toolbar.resume",
  "toolbar.stop",
  "toolbar.delete",
  "toolbar.options",
  "downloads.empty",
  "downloads.error",
  "actions.openFile",
  "actions.openFolder",
  "dropTarget.label",
  "notifications.completed",
  "notifications.failed",
] as const;

export type MessageKey = (typeof MESSAGE_KEYS)[number];
export type SupportedLocale = "en" | "bn";
export type MessageCatalog = Readonly<Record<MessageKey, string>>;

export const ENGLISH_MESSAGES: MessageCatalog = Object.freeze({
  "app.title": "Download Manager",
  "toolbar.addUrl": "Add URL",
  "toolbar.resume": "Resume",
  "toolbar.stop": "Stop",
  "toolbar.delete": "Delete",
  "toolbar.options": "Options",
  "downloads.empty": "No downloads yet",
  "downloads.error": "Downloads could not be loaded",
  "actions.openFile": "Open file",
  "actions.openFolder": "Open containing folder",
  "dropTarget.label": "Drop links to download",
  "notifications.completed": "Download completed",
  "notifications.failed": "Download failed",
});

/** Initial complete catalog; later slices may refine Bangla product wording. */
export const BANGLA_MESSAGES: MessageCatalog = Object.freeze({
  "app.title": "ডাউনলোড ম্যানেজার",
  "toolbar.addUrl": "ইউআরএল যোগ করুন",
  "toolbar.resume": "পুনরায় চালু করুন",
  "toolbar.stop": "থামান",
  "toolbar.delete": "মুছুন",
  "toolbar.options": "বিকল্প",
  "downloads.empty": "এখনও কোনো ডাউনলোড নেই",
  "downloads.error": "ডাউনলোডগুলো লোড করা যায়নি",
  "actions.openFile": "ফাইল খুলুন",
  "actions.openFolder": "ফাইলের ফোল্ডার খুলুন",
  "dropTarget.label": "ডাউনলোড করতে লিংক ছাড়ুন",
  "notifications.completed": "ডাউনলোড সম্পন্ন হয়েছে",
  "notifications.failed": "ডাউনলোড ব্যর্থ হয়েছে",
});

const DEFAULT_CATALOGS: Readonly<Record<SupportedLocale, MessageCatalog>> = {
  en: ENGLISH_MESSAGES,
  bn: BANGLA_MESSAGES,
};

export class LocalizationController {
  readonly #catalogs: Readonly<Record<SupportedLocale, MessageCatalog>>;
  #locale: SupportedLocale;

  public constructor(
    requestedLocale: string,
    catalogs: Readonly<Record<SupportedLocale, unknown>> = DEFAULT_CATALOGS,
  ) {
    this.#catalogs = {
      en: assertCompleteCatalog("en", catalogs.en),
      bn: assertCompleteCatalog("bn", catalogs.bn),
    };
    this.#locale = resolveLocale(requestedLocale);
  }

  public get locale(): SupportedLocale {
    return this.#locale;
  }

  public setLocale(requestedLocale: string): void {
    this.#locale = resolveLocale(requestedLocale);
  }

  public text(key: MessageKey): string {
    return this.#catalogs[this.#locale][key];
  }
}

/** Produces deterministic pseudo-localized copy for 30% text-expansion checks. */
export function createTextExpansionCatalog(
  catalog: MessageCatalog,
  expansion = 0.3,
): MessageCatalog {
  if (!Number.isFinite(expansion) || expansion < 0) {
    throw new RangeError("text expansion must be a non-negative number");
  }
  return Object.fromEntries(
    MESSAGE_KEYS.map((key) => {
      const message = catalog[key];
      const characters = Array.from(message);
      const target = Math.ceil(characters.length * (1 + expansion));
      return [key, `${message}${"~".repeat(target - characters.length)}`];
    }),
  ) as unknown as MessageCatalog;
}

function resolveLocale(requested: string): SupportedLocale {
  return requested.trim().toLocaleLowerCase("en-US").split(/[-_]/u, 1)[0] ===
    "bn"
    ? "bn"
    : "en";
}

function assertCompleteCatalog(
  locale: SupportedLocale,
  value: unknown,
): MessageCatalog {
  if (typeof value !== "object" || value === null) {
    throw new Error(`${locale} message catalog is missing`);
  }
  const catalog = value as Readonly<Record<string, unknown>>;
  for (const key of MESSAGE_KEYS) {
    if (typeof catalog[key] !== "string" || catalog[key].trim().length === 0) {
      throw new Error(`${locale} message catalog is missing ${key}`);
    }
  }
  const unknown = Object.keys(catalog).find(
    (key) => !MESSAGE_KEYS.includes(key as MessageKey),
  );
  if (unknown !== undefined) {
    throw new Error(`${locale} message catalog has unknown key ${unknown}`);
  }
  return Object.freeze({ ...catalog }) as MessageCatalog;
}
