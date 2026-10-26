import type { EngineApi } from "./engine-client.js";
import type { UrlProbe } from "./protocol.js";

export type AddUrlDialogStatus =
  "editing" | "probing" | "ready" | "invalid" | "error";

export interface AddUrlDialogState {
  readonly status: AddUrlDialogStatus;
  readonly url: string;
  readonly destination: string;
  readonly preview: UrlProbe | null;
  readonly errors: {
    readonly url: string | null;
    readonly destination: string | null;
  };
  readonly message: string | null;
}

const emptyErrors = () => ({ url: null, destination: null }) as const;

/** Renderer-facing state for the probe step; durable job creation belongs to S062. */
export class AddUrlDialogController {
  #state: AddUrlDialogState = {
    status: "editing",
    url: "",
    destination: "",
    preview: null,
    errors: emptyErrors(),
    message: null,
  };

  public constructor(private readonly engine: Pick<EngineApi, "probeUrl">) {}

  public get state(): AddUrlDialogState {
    return this.#state;
  }

  public setUrl(url: string): void {
    this.#state = { ...this.#state, status: "editing", url, preview: null };
  }

  public setDestination(destination: string): void {
    this.#state = {
      ...this.#state,
      status: "editing",
      destination,
      preview: null,
    };
  }

  public async probe(): Promise<boolean> {
    const url = this.#state.url.trim();
    const destination = this.#state.destination.trim();
    const errors = {
      url: validateUrl(url),
      destination:
        destination.length === 0 ? "Choose a destination folder" : null,
    };
    if (errors.url !== null || errors.destination !== null) {
      this.#state = {
        status: "invalid",
        url,
        destination,
        preview: null,
        errors,
        message: null,
      };
      return false;
    }
    this.#state = {
      status: "probing",
      url,
      destination,
      preview: null,
      errors: emptyErrors(),
      message: null,
    };
    try {
      const preview = await this.engine.probeUrl(url, destination);
      this.#state = {
        ...this.#state,
        status: "ready",
        preview,
      };
      return true;
    } catch (error) {
      this.#state = {
        ...this.#state,
        status: "error",
        message: error instanceof Error ? error.message : "Unable to probe URL",
      };
      return false;
    }
  }
}

function validateUrl(value: string): string | null {
  if (value.length === 0) return "Enter a URL";
  if (value.length > 8_192) return "URL is too long";
  try {
    const parsed = new URL(value);
    return (parsed.protocol === "http:" || parsed.protocol === "https:") &&
      parsed.hostname.length > 0
      ? null
      : "Enter an HTTP or HTTPS URL";
  } catch {
    return "Enter a valid URL";
  }
}
