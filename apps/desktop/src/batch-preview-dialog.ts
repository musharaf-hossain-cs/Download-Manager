const MAX_BATCH_ITEMS = 128;
const MAX_URL_LENGTH = 8_192;
const MAX_FILENAME_LENGTH = 1_024;
const MAX_QUEUE_NAME_LENGTH = 128;

export interface BatchPreviewInput {
  readonly id: string;
  readonly url: string;
  readonly filename: string | null;
  readonly mimeType: string | null;
  readonly estimatedBytes: number | null;
  readonly sourcePage: string | null;
}

export interface BatchPreviewRow extends BatchPreviewInput {
  readonly checked: boolean;
  readonly errors: {
    readonly url: string | null;
    readonly filename: string | null;
  };
}

export interface BatchQueuedDownload {
  readonly url: string;
  readonly filename: string | null;
  readonly mimeType: string | null;
  readonly estimatedBytes: number | null;
  readonly sourcePage: string | null;
  readonly queueName: string | null;
}

export interface BatchJobPort {
  queueJobs(items: readonly BatchQueuedDownload[]): Promise<readonly string[]>;
}

export interface BatchPreviewDialogState {
  readonly rows: readonly BatchPreviewRow[];
  readonly search: string;
  readonly mimeTypeFilter: string | null;
  readonly queueName: string | null;
}

export interface BatchRowEdit {
  readonly url?: string;
  readonly filename?: string | null;
}

interface MutableBatchRow {
  id: string;
  url: string;
  filename: string | null;
  mimeType: string | null;
  estimatedBytes: number | null;
  sourcePage: string | null;
  checked: boolean;
  errors: { url: string | null; filename: string | null };
}

/** Renderer-neutral controller for reviewing and queueing a native browser batch. */
export class BatchPreviewDialogController {
  readonly #rows: MutableBatchRow[];
  #search = "";
  #mimeTypeFilter: string | null = null;
  #queueName: string | null = null;

  public constructor(
    inputs: readonly BatchPreviewInput[],
    private readonly jobs: BatchJobPort,
  ) {
    if (inputs.length === 0 || inputs.length > MAX_BATCH_ITEMS) {
      throw new RangeError("batch must contain between 1 and 128 items");
    }
    const ids = new Set<string>();
    this.#rows = inputs.map((input) => {
      const id = input.id.trim();
      if (id.length === 0 || ids.has(id)) {
        throw new Error("batch row IDs must be unique and non-empty");
      }
      ids.add(id);
      return createRow({ ...input, id });
    });
  }

  public get state(): BatchPreviewDialogState {
    return {
      rows: this.#rows.map(copyRow),
      search: this.#search,
      mimeTypeFilter: this.#mimeTypeFilter,
      queueName: this.#queueName,
    };
  }

  public get visibleRows(): readonly BatchPreviewRow[] {
    return this.#rows.filter((row) => this.#matchesFilter(row)).map(copyRow);
  }

  public setSearch(search: string): void {
    this.#search = search.trim().toLocaleLowerCase("en-US");
  }

  public setMimeTypeFilter(mimeType: string | null): void {
    this.#mimeTypeFilter = normalizeMimeType(mimeType);
  }

  public setQueueName(queueName: string | null): void {
    if (queueName === null) {
      this.#queueName = null;
      return;
    }
    const normalized = queueName.trim();
    if (normalized.length === 0 || normalized.length > MAX_QUEUE_NAME_LENGTH) {
      throw new Error("queue name must contain between 1 and 128 characters");
    }
    this.#queueName = normalized;
  }

  public editRow(id: string, edit: BatchRowEdit): void {
    const row = this.#findRow(id);
    if (edit.url !== undefined) {
      const result = normalizeHttpUrl(edit.url);
      row.url = result.url ?? edit.url.trim();
      row.errors.url = result.error;
    }
    if (edit.filename !== undefined) {
      const result = normalizeFilename(edit.filename);
      row.filename = result.filename;
      row.errors.filename = result.error;
    }
    if (!isValid(row)) row.checked = false;
  }

  public setChecked(id: string, checked: boolean): void {
    const row = this.#findRow(id);
    if (checked && !isValid(row)) {
      throw new Error("invalid rows cannot be checked");
    }
    row.checked = checked;
  }

  public setVisibleChecked(checked: boolean): void {
    for (const row of this.#rows) {
      if (this.#matchesFilter(row) && (!checked || isValid(row))) {
        row.checked = checked;
      }
    }
  }

  public async queueChecked(): Promise<readonly string[]> {
    const items = this.#rows
      .filter((row) => row.checked && isValid(row))
      .map<BatchQueuedDownload>((row) => ({
        url: requireNormalizedUrl(row.url),
        filename: row.filename,
        mimeType: row.mimeType,
        estimatedBytes: row.estimatedBytes,
        sourcePage: row.sourcePage,
        queueName: this.#queueName,
      }));
    if (items.length === 0) {
      throw new Error("select at least one valid download");
    }
    return this.jobs.queueJobs(items);
  }

  #findRow(id: string): MutableBatchRow {
    const row = this.#rows.find((candidate) => candidate.id === id);
    if (row === undefined) throw new Error(`unknown batch row: ${id}`);
    return row;
  }

  #matchesFilter(row: MutableBatchRow): boolean {
    if (
      this.#mimeTypeFilter !== null &&
      row.mimeType !== this.#mimeTypeFilter
    ) {
      return false;
    }
    if (this.#search.length === 0) return true;
    return [row.url, row.filename, row.mimeType, row.sourcePage].some((value) =>
      value?.toLocaleLowerCase("en-US").includes(this.#search),
    );
  }
}

function createRow(input: BatchPreviewInput): MutableBatchRow {
  const url = normalizeHttpUrl(input.url);
  const filename = normalizeFilename(input.filename);
  const mimeType = normalizeMimeType(input.mimeType);
  const row: MutableBatchRow = {
    id: input.id,
    url: url.url ?? input.url.trim(),
    filename: filename.filename,
    mimeType,
    estimatedBytes: normalizeEstimatedBytes(input.estimatedBytes),
    sourcePage: normalizeOptionalHttpUrl(input.sourcePage),
    checked: false,
    errors: { url: url.error, filename: filename.error },
  };
  row.checked = isValid(row);
  return row;
}

function copyRow(row: MutableBatchRow): BatchPreviewRow {
  return { ...row, errors: { ...row.errors } };
}

function isValid(row: MutableBatchRow): boolean {
  return row.errors.url === null && row.errors.filename === null;
}

function normalizeHttpUrl(value: string): {
  readonly url: string | null;
  readonly error: string | null;
} {
  const candidate = value.trim();
  if (candidate.length === 0 || candidate.length > MAX_URL_LENGTH) {
    return { url: null, error: "Enter a valid HTTP(S) URL" };
  }
  try {
    const url = new URL(candidate);
    if (
      (url.protocol !== "http:" && url.protocol !== "https:") ||
      url.hostname.length === 0
    ) {
      return { url: null, error: "Enter a valid HTTP(S) URL" };
    }
    url.hash = "";
    return { url: url.toString(), error: null };
  } catch {
    return { url: null, error: "Enter a valid HTTP(S) URL" };
  }
}

function requireNormalizedUrl(value: string): string {
  const result = normalizeHttpUrl(value);
  if (result.url === null) throw new Error("checked row has an invalid URL");
  return result.url;
}

function normalizeOptionalHttpUrl(value: string | null): string | null {
  if (value === null) return null;
  return normalizeHttpUrl(value).url;
}

function normalizeFilename(value: string | null): {
  readonly filename: string | null;
  readonly error: string | null;
} {
  if (value === null) return { filename: null, error: null };
  const filename = value.trim();
  return filename.length === 0 ||
    filename.length > MAX_FILENAME_LENGTH ||
    /[\0\r\n]/u.test(filename)
    ? { filename, error: "Enter a valid filename" }
    : { filename, error: null };
}

function normalizeMimeType(value: string | null): string | null {
  const mimeType = value?.split(";", 1)[0]?.trim().toLocaleLowerCase("en-US");
  return mimeType === undefined || mimeType.length === 0 ? null : mimeType;
}

function normalizeEstimatedBytes(value: number | null): number | null {
  return value !== null && Number.isSafeInteger(value) && value >= 0
    ? value
    : null;
}
