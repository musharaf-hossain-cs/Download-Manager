import { AddUrlDialogController } from "./add-url-dialog.js";
import { DownloadListController } from "./download-list.js";
import { EngineClient } from "./engine-client.js";
import { presentJob, formatBytes } from "./private-use-presenter.js";
import type { DownloadJob } from "./protocol.js";
import { SegmentDetailController } from "./segment-detail.js";
import { TauriEngineTransport, type TauriInvoke } from "./tauri-transport.js";

export const JOB_POLL_INTERVAL_MS = 500;

interface TauriGlobal {
  readonly core: {
    invoke<T>(
      command: string,
      arguments_?: Record<string, unknown>,
    ): Promise<T>;
  };
  readonly dialog: {
    open(options: {
      readonly directory: boolean;
      readonly multiple: false;
      readonly title: string;
    }): Promise<string | readonly string[] | null>;
  };
}

declare global {
  // Tauri installs this only inside the application webview.
  var __TAURI__: TauriGlobal | undefined;
}

type ChooseDestination = () => Promise<string | null>;

export class PrivateUseApp {
  readonly #engine: EngineClient;
  readonly #addController: AddUrlDialogController;
  readonly #listController: DownloadListController;
  readonly #details = new Map<string, SegmentDetailController>();
  readonly #statusCard: HTMLElement;
  readonly #statusTitle: HTMLElement;
  readonly #statusMessage: HTMLElement;
  readonly #backendVersion: HTMLElement;
  readonly #addButton: HTMLButtonElement;
  readonly #jobs: HTMLElement;
  readonly #listMessage: HTMLElement;
  readonly #dialog: HTMLDialogElement;
  readonly #form: HTMLFormElement;
  readonly #urlInput: HTMLInputElement;
  readonly #destinationInput: HTMLInputElement;
  readonly #urlError: HTMLElement;
  readonly #destinationError: HTMLElement;
  readonly #addMessageElement: HTMLElement;
  readonly #preview: HTMLElement;
  readonly #previewFilename: HTMLElement;
  readonly #previewPath: HTMLElement;
  readonly #previewSize: HTMLElement;
  readonly #submitButton: HTMLButtonElement;
  #timer: number | null = null;
  #stopped = false;
  #dialogBusy = false;
  #addMessage: string | null = null;

  public constructor(
    private readonly document: Document,
    transport: TauriEngineTransport,
    private readonly chooseDestination: ChooseDestination,
  ) {
    this.#engine = new EngineClient(transport);
    this.#addController = new AddUrlDialogController(this.#engine);
    this.#listController = new DownloadListController(this.#engine);
    this.#statusCard = required(document, ".status-card");
    this.#statusTitle = required(document, "#status-title");
    this.#statusMessage = required(document, "#status-message");
    this.#backendVersion = required(document, "#backend-version");
    this.#addButton = required(document, "#add-download");
    this.#jobs = required(document, "#jobs");
    this.#listMessage = required(document, "#list-message");
    this.#dialog = required(document, "#add-dialog");
    this.#form = required(document, "#add-form");
    this.#urlInput = required(document, "#download-url");
    this.#destinationInput = required(document, "#destination");
    this.#urlError = required(document, "#url-error");
    this.#destinationError = required(document, "#destination-error");
    this.#addMessageElement = required(document, "#add-message");
    this.#preview = required(document, "#url-preview");
    this.#previewFilename = required(document, "#preview-filename");
    this.#previewPath = required(document, "#preview-path");
    this.#previewSize = required(document, "#preview-size");
    this.#submitButton = required(document, "#add-submit");
    this.#bindEvents();
  }

  public async start(): Promise<void> {
    try {
      const version = await this.#engine.health();
      this.#statusCard.dataset.state = "ready";
      this.#statusTitle.textContent = "Ready";
      this.#statusMessage.textContent =
        "Connected to the current-user download backend.";
      this.#backendVersion.textContent = `Backend ${version}`;
      this.#backendVersion.hidden = false;
      this.#addButton.disabled = false;
      await this.#refreshJobs();
      this.#schedulePoll();
    } catch (error) {
      this.#showFailure(
        errorMessage(error, "Unable to connect to the backend"),
      );
    }
  }

  public async stop(): Promise<void> {
    this.#stopped = true;
    if (this.#timer !== null) globalThis.clearTimeout(this.#timer);
    this.#timer = null;
    await this.#engine.close();
  }

  #bindEvents(): void {
    this.#addButton.addEventListener("click", () => {
      this.#addMessage = null;
      this.#renderDialog();
      this.#dialog.showModal();
      this.#urlInput.focus();
    });
    required<HTMLButtonElement>(this.document, "#add-cancel").addEventListener(
      "click",
      () => this.#dialog.close(),
    );
    required<HTMLButtonElement>(
      this.document,
      "#add-cancel-secondary",
    ).addEventListener("click", () => this.#dialog.close());
    required<HTMLButtonElement>(
      this.document,
      "#choose-destination",
    ).addEventListener("click", () => void this.#chooseFolder());
    this.#urlInput.addEventListener("input", () => {
      this.#addController.setUrl(this.#urlInput.value);
      this.#addMessage = null;
      this.#renderDialog();
    });
    this.#destinationInput.addEventListener("input", () => {
      this.#addController.setDestination(this.#destinationInput.value);
      this.#addMessage = null;
      this.#renderDialog();
    });
    this.#form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.#submitDialog();
    });
  }

  async #chooseFolder(): Promise<void> {
    try {
      const destination = await this.chooseDestination();
      if (destination === null) return;
      this.#destinationInput.value = destination;
      this.#addController.setDestination(destination);
      this.#addMessage = null;
      this.#renderDialog();
    } catch (error) {
      this.#addMessage = errorMessage(error, "Could not open folder picker");
      this.#renderDialog();
    }
  }

  async #submitDialog(): Promise<void> {
    if (this.#dialogBusy) return;
    this.#dialogBusy = true;
    this.#addMessage = null;
    this.#renderDialog();
    try {
      if (this.#addController.state.status !== "ready") {
        await this.#addController.probe();
        return;
      }
      await this.#addController.submit("now", "add_duplicate");
      this.#addController.setUrl("");
      this.#urlInput.value = "";
      this.#dialog.close();
      await this.#refreshJobs();
    } catch (error) {
      this.#addMessage = errorMessage(error, "Could not add the download");
    } finally {
      this.#dialogBusy = false;
      this.#renderDialog();
    }
  }

  #renderDialog(): void {
    const state = this.#addController.state;
    this.#urlError.textContent = state.errors.url ?? "";
    this.#destinationError.textContent = state.errors.destination ?? "";
    this.#addMessageElement.textContent =
      this.#addMessage ?? state.message ?? "";
    this.#addMessageElement.hidden =
      this.#addMessage === null && state.message === null;
    const preview = state.preview;
    this.#preview.hidden = preview === null;
    if (preview !== null) {
      this.#previewFilename.textContent = preview.filename;
      this.#previewPath.textContent = preview.destination;
      this.#previewSize.textContent =
        preview.total_bytes === null
          ? "Size not reported"
          : formatBytes(preview.total_bytes);
    }
    this.#submitButton.disabled = this.#dialogBusy;
    this.#submitButton.textContent = this.#dialogBusy
      ? state.status === "ready"
        ? "Starting…"
        : "Checking…"
      : state.status === "ready"
        ? "Start download"
        : "Check URL";
  }

  async #refreshJobs(): Promise<void> {
    await this.#listController.load();
    this.#renderJobs();
  }

  #schedulePoll(): void {
    if (this.#stopped) return;
    this.#timer = globalThis.setTimeout(() => {
      void this.#poll();
    }, JOB_POLL_INTERVAL_MS);
  }

  async #poll(): Promise<void> {
    try {
      await this.#refreshJobs();
    } finally {
      this.#schedulePoll();
    }
  }

  #renderJobs(): void {
    const state = this.#listController.state;
    this.#jobs.replaceChildren();
    if (state.status === "empty") {
      this.#listMessage.textContent =
        "No downloads yet. Add an HTTP or HTTPS URL to get started.";
      this.#listMessage.hidden = false;
      return;
    }
    if (state.status === "error") {
      this.#listMessage.textContent = `Downloads could not be refreshed: ${state.message}`;
      this.#listMessage.hidden = false;
      return;
    }
    if (state.status !== "ready") return;
    this.#listMessage.hidden = true;
    const now = Date.now();
    for (const job of state.jobs) this.#jobs.append(this.#jobCard(job, now));
  }

  #jobCard(job: DownloadJob, at: number): HTMLElement {
    let detailController = this.#details.get(job.id);
    if (detailController === undefined) {
      detailController = new SegmentDetailController();
      this.#details.set(job.id, detailController);
    }
    const detail = detailController.apply({
      job_id: job.id,
      at,
      downloaded_bytes: job.downloaded_bytes,
      total_bytes: job.total_bytes,
      segments: [],
    });
    const view = presentJob(job, detail);
    const article = this.document.createElement("article");
    article.className = "job-card";
    article.dataset.state = job.state;

    const header = this.document.createElement("div");
    header.className = "job-heading";
    const title = this.document.createElement("h3");
    title.textContent = job.filename;
    const status = this.document.createElement("span");
    status.className = "job-status";
    status.textContent = view.status;
    header.append(title, status);

    const destination = this.document.createElement("p");
    destination.className = "job-destination";
    destination.textContent = job.destination;
    destination.title = job.destination;

    const progress = this.document.createElement("progress");
    progress.max = 100;
    if (view.progressPercent !== null) progress.value = view.progressPercent;
    progress.setAttribute(
      "aria-label",
      `${job.filename}: ${view.progressLabel}`,
    );

    const metrics = this.document.createElement("dl");
    metrics.className = "job-metrics";
    addMetric(this.document, metrics, "Transferred", view.transferLabel);
    addMetric(this.document, metrics, "Speed", view.speedLabel);
    addMetric(this.document, metrics, "ETA", view.etaLabel);
    if (job.error !== null) {
      const error = this.document.createElement("p");
      error.className = "job-error";
      error.textContent = job.error;
      article.append(header, destination, progress, metrics, error);
    } else {
      article.append(header, destination, progress, metrics);
    }
    return article;
  }

  #showFailure(message: string): void {
    this.#statusCard.dataset.state = "error";
    this.#statusCard.setAttribute("role", "alert");
    this.#statusTitle.textContent = "The local backend could not start";
    this.#statusMessage.textContent = message;
    this.#backendVersion.hidden = true;
    this.#addButton.disabled = true;
  }
}

function required<T extends Element = HTMLElement>(
  document: Document,
  selector: string,
): T {
  const element = document.querySelector<T>(selector);
  if (element === null)
    throw new Error(`Missing desktop UI element: ${selector}`);
  return element;
}

function addMetric(
  document: Document,
  list: HTMLDListElement,
  term: string,
  value: string,
): void {
  const metric = document.createElement("div");
  const dt = document.createElement("dt");
  dt.textContent = term;
  const dd = document.createElement("dd");
  dd.textContent = value;
  metric.append(dt, dd);
  list.append(metric);
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

const tauri = globalThis.__TAURI__;
if (tauri !== undefined) {
  const invoke: TauriInvoke = (command, arguments_) =>
    tauri.core.invoke(command, arguments_);
  const transport = new TauriEngineTransport(invoke);
  const app = new PrivateUseApp(globalThis.document, transport, async () => {
    const selected = await tauri.dialog.open({
      directory: true,
      multiple: false,
      title: "Choose download destination",
    });
    return typeof selected === "string" ? selected : null;
  });
  void app.start();
  globalThis.addEventListener("beforeunload", () => void app.stop(), {
    once: true,
  });
}
