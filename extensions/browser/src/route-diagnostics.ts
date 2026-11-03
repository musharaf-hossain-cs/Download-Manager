export type ObservedProxyMode = "direct" | "system" | "manual" | "unknown";

export interface RouteObservation {
  /**
   * An opaque, short-lived value returned by the same route-check service.
   * Adapters must not place an IP address or proxy credentials in this field.
   */
  readonly egressFingerprint: string | null;
  readonly proxyMode: ObservedProxyMode;
}

export interface BrowserNativeRouteProbe {
  observeBrowserRoute(): Promise<RouteObservation>;
  observeNativeRoute(): Promise<RouteObservation>;
}

export type RouteCompatibility = "matched" | "mismatch" | "indeterminate";

export interface BrowserNativeRouteReport {
  readonly compatibility: RouteCompatibility;
  readonly browser: RouteObservation | null;
  readonly native: RouteObservation | null;
  readonly limitation: string;
  readonly explanation: string;
  readonly recommendedAction: string;
}

export interface RouteDiagnosticPresenter {
  showRouteDiagnostic(report: BrowserNativeRouteReport): void;
}

export const BROWSER_ROUTE_INHERITANCE_LIMITATION =
  "Download Manager cannot directly inherit the browser's proxy or VPN route for a native download.";

/**
 * Compares bounded observations made through the browser and native network
 * stacks. It detects evidence of a mismatch; it never claims that a matching
 * sample guarantees identical routing for a later download.
 */
export class BrowserNativeRouteDiagnostics {
  public constructor(
    private readonly probe: BrowserNativeRouteProbe,
    private readonly presenter: RouteDiagnosticPresenter,
  ) {}

  public async check(): Promise<BrowserNativeRouteReport> {
    const [browserResult, nativeResult] = await Promise.allSettled([
      this.probe.observeBrowserRoute(),
      this.probe.observeNativeRoute(),
    ]);
    const browser = observation(browserResult);
    const native = observation(nativeResult);
    const report = evaluateBrowserNativeRoute(browser, native);
    this.presenter.showRouteDiagnostic(report);
    return report;
  }
}

export function evaluateBrowserNativeRoute(
  browser: RouteObservation | null,
  native: RouteObservation | null,
): BrowserNativeRouteReport {
  const browserObservation = sanitizeObservation(browser);
  const nativeObservation = sanitizeObservation(native);
  const compatibility = compare(browserObservation, nativeObservation);

  if (compatibility === "mismatch") {
    return report(
      compatibility,
      browserObservation,
      nativeObservation,
      "The browser and native app reported different proxy modes or network-route fingerprints.",
      "Keep this download in the browser, or align the app proxy/VPN settings and check again.",
    );
  }
  if (compatibility === "matched") {
    return report(
      compatibility,
      browserObservation,
      nativeObservation,
      "The current browser and native route samples match, but a later request can still take a different route.",
      "You can continue, and re-run diagnostics after changing proxy or VPN settings.",
    );
  }
  return report(
    compatibility,
    browserObservation,
    nativeObservation,
    "The extension could not compare both routes, so route compatibility is unknown.",
    "Check the browser and app proxy/VPN settings before handing off the download.",
  );
}

function observation(
  result: PromiseSettledResult<RouteObservation>,
): RouteObservation | null {
  return result.status === "fulfilled" ? result.value : null;
}

function sanitizeObservation(
  value: RouteObservation | null,
): RouteObservation | null {
  if (value === null || !isProxyMode(value.proxyMode)) return null;
  const egressFingerprint = value.egressFingerprint;
  if (
    egressFingerprint !== null &&
    !/^[A-Za-z0-9_-]{16,128}$/u.test(egressFingerprint)
  ) {
    return null;
  }
  return { proxyMode: value.proxyMode, egressFingerprint };
}

function compare(
  browser: RouteObservation | null,
  native: RouteObservation | null,
): RouteCompatibility {
  if (browser === null || native === null) return "indeterminate";

  if (
    browser.proxyMode !== "unknown" &&
    native.proxyMode !== "unknown" &&
    browser.proxyMode !== native.proxyMode
  ) {
    return "mismatch";
  }
  if (browser.egressFingerprint !== null && native.egressFingerprint !== null) {
    return browser.egressFingerprint === native.egressFingerprint
      ? "matched"
      : "mismatch";
  }
  return "indeterminate";
}

function report(
  compatibility: RouteCompatibility,
  browser: RouteObservation | null,
  native: RouteObservation | null,
  explanation: string,
  recommendedAction: string,
): BrowserNativeRouteReport {
  return {
    compatibility,
    browser,
    native,
    limitation: BROWSER_ROUTE_INHERITANCE_LIMITATION,
    explanation,
    recommendedAction,
  };
}

function isProxyMode(value: unknown): value is ObservedProxyMode {
  return (
    value === "direct" ||
    value === "system" ||
    value === "manual" ||
    value === "unknown"
  );
}
