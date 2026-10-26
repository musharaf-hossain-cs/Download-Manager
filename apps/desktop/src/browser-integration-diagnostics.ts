export type ChromiumBrowser = "chrome" | "edge";

export type BrowserIntegrationCheckStatus = "pass" | "fail" | "blocked";

export type BrowserIntegrationRepairAction =
  | "install_extension"
  | "update_extension"
  | "repair_registration"
  | "reinstall_application";

export interface NativeHostManifestSnapshot {
  readonly name: string;
  readonly executablePath: string;
  readonly allowedExtensionIds: readonly string[];
}

export interface BrowserIntegrationSnapshot {
  readonly appVersion: string;
  readonly extension: {
    readonly installed: boolean;
    readonly id: string | null;
    readonly version: string | null;
  };
  readonly nativeHost: {
    readonly registeredBrowsers: readonly ChromiumBrowser[];
    readonly manifest: NativeHostManifestSnapshot | null;
    readonly executableExists: boolean;
    readonly reachable: boolean;
    readonly protocolVersion: number | null;
    readonly version: string | null;
  };
}

export interface BrowserIntegrationExpectations {
  readonly appVersion: string;
  readonly extensionId: string;
  readonly nativeHostName: string;
  readonly protocolVersion: number;
  readonly browsers: readonly ChromiumBrowser[];
}

export interface BrowserIntegrationCheck {
  readonly id: string;
  readonly title: string;
  readonly status: BrowserIntegrationCheckStatus;
  readonly detail: string;
  readonly recommendedAction: string | null;
  readonly repairAction: BrowserIntegrationRepairAction | null;
}

export interface BrowserIntegrationReport {
  readonly status: "healthy" | "action_required";
  readonly summary: string;
  readonly checks: readonly BrowserIntegrationCheck[];
  readonly repairActions: readonly BrowserIntegrationRepairAction[];
}

export interface BrowserIntegrationPort {
  inspect(): Promise<BrowserIntegrationSnapshot>;
  repair(actions: readonly BrowserIntegrationRepairAction[]): Promise<void>;
}

export type BrowserIntegrationDiagnosticsStatus =
  "idle" | "checking" | "ready" | "repairing" | "error";

export interface BrowserIntegrationDiagnosticsState {
  readonly status: BrowserIntegrationDiagnosticsStatus;
  readonly report: BrowserIntegrationReport | null;
  readonly error: string | null;
  readonly canRepair: boolean;
}

/** Renderer-neutral state for the browser integration self-check/repair screen. */
export class BrowserIntegrationDiagnosticsController {
  #status: BrowserIntegrationDiagnosticsStatus = "idle";
  #report: BrowserIntegrationReport | null = null;
  #error: string | null = null;

  public constructor(
    private readonly expectations: BrowserIntegrationExpectations,
    private readonly port: BrowserIntegrationPort,
  ) {
    validateExpectations(expectations);
  }

  public get state(): BrowserIntegrationDiagnosticsState {
    return {
      status: this.#status,
      report: this.#report === null ? null : copyReport(this.#report),
      error: this.#error,
      canRepair: (this.#report?.repairActions.length ?? 0) > 0,
    };
  }

  public async runSelfCheck(): Promise<BrowserIntegrationReport> {
    this.#status = "checking";
    this.#error = null;
    try {
      const snapshot = await this.port.inspect();
      this.#report = diagnoseBrowserIntegration(snapshot, this.expectations);
      this.#status = "ready";
      return copyReport(this.#report);
    } catch (error) {
      this.#status = "error";
      this.#error = safeErrorMessage(error);
      throw error;
    }
  }

  /** Runs every deduplicated repair action, then verifies the resulting state. */
  public async repair(): Promise<BrowserIntegrationReport> {
    if (this.#report === null) {
      throw new Error("run the browser integration self-check first");
    }
    if (this.#report.repairActions.length === 0) {
      return copyReport(this.#report);
    }
    const previous = this.#report;
    this.#status = "repairing";
    this.#error = null;
    try {
      await this.port.repair([...previous.repairActions]);
      const snapshot = await this.port.inspect();
      this.#report = diagnoseBrowserIntegration(snapshot, this.expectations);
      this.#status = "ready";
      return copyReport(this.#report);
    } catch (error) {
      this.#report = previous;
      this.#status = "error";
      this.#error = safeErrorMessage(error);
      throw error;
    }
  }
}

/** Converts one bounded local inspection into stable, actionable check rows. */
export function diagnoseBrowserIntegration(
  snapshot: BrowserIntegrationSnapshot,
  expectations: BrowserIntegrationExpectations,
): BrowserIntegrationReport {
  validateExpectations(expectations);
  const checks: BrowserIntegrationCheck[] = [];
  checks.push(
    equalityCheck(
      "app.version",
      "Application version",
      snapshot.appVersion,
      expectations.appVersion,
      "Reinstall Download Manager to restore the expected application version.",
      "reinstall_application",
    ),
  );

  if (!snapshot.extension.installed) {
    checks.push(
      failure(
        "extension.installation",
        "Browser extension",
        "The Download Manager extension is not installed.",
        "Install the packaged Download Manager extension, then run the check again.",
        "install_extension",
      ),
      blocked(
        "extension.identity",
        "Extension identity",
        "Install the extension before its identity can be checked.",
      ),
      blocked(
        "extension.version",
        "Extension version",
        "Install the extension before its version can be checked.",
      ),
    );
  } else {
    checks.push(
      equalityCheck(
        "extension.identity",
        "Extension identity",
        snapshot.extension.id,
        expectations.extensionId,
        "Install the expected packaged extension and remove the unrecognized copy.",
        "install_extension",
      ),
      equalityCheck(
        "extension.version",
        "Extension version",
        snapshot.extension.version,
        expectations.appVersion,
        "Update the browser extension so its version matches the application.",
        "update_extension",
      ),
    );
  }

  const registrations = new Set(snapshot.nativeHost.registeredBrowsers);
  for (const browser of uniqueBrowsers(expectations.browsers)) {
    checks.push(
      registrations.has(browser)
        ? passing(
            `registration.${browser}`,
            `${browserName(browser)} registration`,
            `The native host is registered for ${browserName(browser)}.`,
          )
        : failure(
            `registration.${browser}`,
            `${browserName(browser)} registration`,
            `The native-host registration for ${browserName(browser)} is missing.`,
            `Repair the current-user ${browserName(browser)} native-host registration.`,
            "repair_registration",
          ),
    );
  }

  const manifest = snapshot.nativeHost.manifest;
  if (manifest === null) {
    checks.push(
      failure(
        "native_host.manifest",
        "Native-host manifest",
        "The native-host manifest is missing or unreadable.",
        "Recreate the current-user native-host manifest and registrations.",
        "repair_registration",
      ),
      blocked(
        "native_host.executable",
        "Native-host executable",
        "Restore the native-host manifest before checking its executable.",
      ),
      blocked(
        "native_host.allowlist",
        "Extension allowlist",
        "Restore the native-host manifest before checking its extension allowlist.",
      ),
    );
  } else {
    checks.push(
      equalityCheck(
        "native_host.manifest",
        "Native-host manifest",
        manifest.name,
        expectations.nativeHostName,
        "Replace the manifest with the Download Manager native-host manifest.",
        "repair_registration",
      ),
      snapshot.nativeHost.executableExists
        ? passing(
            "native_host.executable",
            "Native-host executable",
            `The registered executable exists at ${manifest.executablePath}.`,
          )
        : failure(
            "native_host.executable",
            "Native-host executable",
            `The registered executable is missing at ${manifest.executablePath}.`,
            "Reinstall Download Manager to restore the native-host executable.",
            "reinstall_application",
          ),
      manifest.allowedExtensionIds.includes(expectations.extensionId)
        ? passing(
            "native_host.allowlist",
            "Extension allowlist",
            "The packaged extension ID is allowed by the native host.",
          )
        : failure(
            "native_host.allowlist",
            "Extension allowlist",
            "The packaged extension ID is absent from the native-host allowlist.",
            "Repair registration to bind the manifest to this extension.",
            "repair_registration",
          ),
    );
  }

  if (!snapshot.nativeHost.reachable) {
    checks.push(
      failure(
        "native_host.connection",
        "Native-host connection",
        "The browser could not complete a native-host health check.",
        "Repair registration and restart the browser before retrying.",
        "repair_registration",
      ),
      blocked(
        "protocol.version",
        "Messaging protocol",
        "Connect to the native host before checking the protocol version.",
      ),
      blocked(
        "native_host.version",
        "Native-host version",
        "Connect to the native host before checking its version.",
      ),
    );
  } else {
    checks.push(
      passing(
        "native_host.connection",
        "Native-host connection",
        "The browser completed the native-host health check.",
      ),
      numericEqualityCheck(
        "protocol.version",
        "Messaging protocol",
        snapshot.nativeHost.protocolVersion,
        expectations.protocolVersion,
        "Update both the application and extension to compatible versions.",
      ),
      equalityCheck(
        "native_host.version",
        "Native-host version",
        snapshot.nativeHost.version,
        expectations.appVersion,
        "Reinstall Download Manager so the native host matches the application.",
        "reinstall_application",
      ),
    );
  }

  const repairActions = [
    ...new Set(
      checks.flatMap((check) =>
        check.status === "fail" && check.repairAction !== null
          ? [check.repairAction]
          : [],
      ),
    ),
  ];
  const failed = checks.filter((check) => check.status === "fail").length;
  return {
    status: failed === 0 ? "healthy" : "action_required",
    summary:
      failed === 0
        ? "Browser integration is healthy."
        : `${failed} browser integration ${failed === 1 ? "check needs" : "checks need"} attention.`,
    checks,
    repairActions,
  };
}

function equalityCheck(
  id: string,
  title: string,
  actual: string | null,
  expected: string,
  action: string,
  repairAction: BrowserIntegrationRepairAction,
): BrowserIntegrationCheck {
  return actual === expected
    ? passing(id, title, `Expected value ${expected} is present.`)
    : failure(
        id,
        title,
        `Found ${actual ?? "no value"}; expected ${expected}.`,
        action,
        repairAction,
      );
}

function numericEqualityCheck(
  id: string,
  title: string,
  actual: number | null,
  expected: number,
  action: string,
): BrowserIntegrationCheck {
  return actual === expected
    ? passing(id, title, `Protocol ${expected} is compatible.`)
    : failure(
        id,
        title,
        `Found protocol ${actual ?? "unknown"}; expected ${expected}.`,
        action,
        "reinstall_application",
      );
}

function passing(
  id: string,
  title: string,
  detail: string,
): BrowserIntegrationCheck {
  return {
    id,
    title,
    status: "pass",
    detail,
    recommendedAction: null,
    repairAction: null,
  };
}

function failure(
  id: string,
  title: string,
  detail: string,
  recommendedAction: string,
  repairAction: BrowserIntegrationRepairAction,
): BrowserIntegrationCheck {
  return {
    id,
    title,
    status: "fail",
    detail,
    recommendedAction,
    repairAction,
  };
}

function blocked(
  id: string,
  title: string,
  detail: string,
): BrowserIntegrationCheck {
  return {
    id,
    title,
    status: "blocked",
    detail,
    recommendedAction: null,
    repairAction: null,
  };
}

function uniqueBrowsers(
  browsers: readonly ChromiumBrowser[],
): readonly ChromiumBrowser[] {
  return [...new Set(browsers)];
}

function browserName(browser: ChromiumBrowser): string {
  return browser === "chrome" ? "Google Chrome" : "Microsoft Edge";
}

function validateExpectations(
  expectations: BrowserIntegrationExpectations,
): void {
  if (
    expectations.appVersion.trim().length === 0 ||
    expectations.nativeHostName.trim().length === 0 ||
    expectations.protocolVersion < 1 ||
    !Number.isSafeInteger(expectations.protocolVersion) ||
    !/^[a-p]{32}$/u.test(expectations.extensionId) ||
    expectations.browsers.length === 0
  ) {
    throw new Error("invalid browser integration expectations");
  }
}

function copyReport(
  report: BrowserIntegrationReport,
): BrowserIntegrationReport {
  return {
    ...report,
    checks: report.checks.map((check) => ({ ...check })),
    repairActions: [...report.repairActions],
  };
}

function safeErrorMessage(error: unknown): string {
  return error instanceof Error && error.message.trim().length > 0
    ? error.message
    : "Browser integration diagnostics failed.";
}
