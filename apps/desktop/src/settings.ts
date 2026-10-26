export type SettingValue = string | number | boolean | null;

export interface SettingDefinition {
  readonly section: string;
  readonly key: string;
  readonly label: string;
  readonly description: string;
  readonly keywords?: readonly string[];
  readonly defaultValue: SettingValue;
}

export type SettingsSnapshot = Readonly<
  Record<string, Readonly<Record<string, SettingValue>>>
>;

export interface SettingsStore {
  /** Persists preferences only; jobs and downloaded files are outside this port. */
  save(settings: SettingsSnapshot): Promise<void>;
}

export interface SettingSearchResult extends SettingDefinition {
  readonly value: SettingValue;
  readonly isDefault: boolean;
}

/** Searchable settings state with preference-only reset operations. */
export class SettingsController {
  readonly #definitions: readonly SettingDefinition[];
  readonly #store: SettingsStore;
  #values: Record<string, Record<string, SettingValue>>;

  public constructor(
    definitions: readonly SettingDefinition[],
    persisted: SettingsSnapshot,
    store: SettingsStore,
  ) {
    assertUniqueDefinitions(definitions);
    this.#definitions = definitions.map((definition) => ({ ...definition }));
    this.#store = store;
    this.#values = valuesFrom(definitions, persisted);
  }

  public get values(): SettingsSnapshot {
    return cloneValues(this.#values);
  }

  public search(query: string): readonly SettingSearchResult[] {
    const words = query
      .trim()
      .toLocaleLowerCase()
      .split(/\s+/u)
      .filter(Boolean);
    return this.#definitions
      .filter((definition) => {
        const haystack = [
          definition.section,
          definition.key,
          definition.label,
          definition.description,
          ...(definition.keywords ?? []),
        ]
          .join(" ")
          .toLocaleLowerCase();
        return words.every((word) => haystack.includes(word));
      })
      .map((definition) => {
        const value = this.#values[definition.section]?.[definition.key];
        return {
          ...definition,
          value: value ?? definition.defaultValue,
          isDefault: value === definition.defaultValue,
        };
      });
  }

  public async setValue(
    section: string,
    key: string,
    value: SettingValue,
  ): Promise<void> {
    const definition = this.#find(section, key);
    if (!sameValueType(value, definition.defaultValue)) {
      throw new TypeError(`invalid value type for ${section}.${key}`);
    }
    const previous = this.#values;
    this.#values = {
      ...this.#values,
      [section]: { ...this.#values[section], [key]: value },
    };
    await this.#saveOrRestore(previous);
  }

  public async resetSection(section: string): Promise<void> {
    const definitions = this.#definitions.filter(
      (definition) => definition.section === section,
    );
    if (definitions.length === 0)
      throw new RangeError(`unknown section: ${section}`);
    const previous = this.#values;
    this.#values = {
      ...this.#values,
      [section]: Object.fromEntries(
        definitions.map((definition) => [
          definition.key,
          definition.defaultValue,
        ]),
      ),
    };
    await this.#saveOrRestore(previous);
  }

  public async resetAll(): Promise<void> {
    const previous = this.#values;
    this.#values = valuesFrom(this.#definitions, {});
    await this.#saveOrRestore(previous);
  }

  #find(section: string, key: string): SettingDefinition {
    const definition = this.#definitions.find(
      (candidate) => candidate.section === section && candidate.key === key,
    );
    if (definition === undefined)
      throw new RangeError(`unknown setting: ${section}.${key}`);
    return definition;
  }

  async #saveOrRestore(previous: Record<string, Record<string, SettingValue>>) {
    try {
      await this.#store.save(this.values);
    } catch (error) {
      this.#values = previous;
      throw error;
    }
  }
}

function valuesFrom(
  definitions: readonly SettingDefinition[],
  persisted: SettingsSnapshot,
): Record<string, Record<string, SettingValue>> {
  const values: Record<string, Record<string, SettingValue>> = {};
  for (const definition of definitions) {
    const candidate = persisted[definition.section]?.[definition.key];
    (values[definition.section] ??= {})[definition.key] =
      candidate !== undefined &&
      sameValueType(candidate, definition.defaultValue)
        ? candidate
        : definition.defaultValue;
  }
  return values;
}

function cloneValues(
  values: Record<string, Record<string, SettingValue>>,
): SettingsSnapshot {
  return Object.fromEntries(
    Object.entries(values).map(([section, sectionValues]) => [
      section,
      { ...sectionValues },
    ]),
  );
}

function sameValueType(value: SettingValue, reference: SettingValue): boolean {
  return reference === null || value === null
    ? value === reference
    : typeof value === typeof reference;
}

function assertUniqueDefinitions(
  definitions: readonly SettingDefinition[],
): void {
  const identities = definitions.map(
    ({ section, key }) => `${section}\0${key}`,
  );
  if (new Set(identities).size !== identities.length)
    throw new Error("setting definitions must be unique");
}
