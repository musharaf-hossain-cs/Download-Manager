export const SUPPORTED_BROWSER_IDS = [
  "chrome",
  "edge",
  "firefox",
  "brave",
  "opera",
] as const;

export type BrowserId = (typeof SUPPORTED_BROWSER_IDS)[number];

export interface BrowserProfileIdentity {
  readonly browser: BrowserId;
  /** Opaque ID generated and retained by that browser profile. */
  readonly profileId: string;
}

export interface BrowserProfileScope extends BrowserProfileIdentity {
  /** Collision-free storage key for the browser/profile pair. */
  readonly key: string;
}

export interface ProfileMetadataPort<Metadata> {
  load(scope: BrowserProfileScope): Promise<Metadata | null>;
  save(scope: BrowserProfileScope, metadata: Metadata): Promise<void>;
}

/**
 * Binds metadata access to exactly one browser profile. Callers cannot select a
 * different profile per operation, which keeps request and secret references
 * inside the profile that produced them.
 */
export class BrowserProfileMetadataBoundary<Metadata> {
  readonly #scope: BrowserProfileScope;

  public constructor(
    identity: BrowserProfileIdentity,
    private readonly port: ProfileMetadataPort<Metadata>,
  ) {
    this.#scope = createBrowserProfileScope(identity);
  }

  public get identity(): BrowserProfileIdentity {
    return {
      browser: this.#scope.browser,
      profileId: this.#scope.profileId,
    };
  }

  public load(): Promise<Metadata | null> {
    return this.port.load(this.#scope);
  }

  public save(metadata: Metadata): Promise<void> {
    return this.port.save(this.#scope, metadata);
  }
}

export type BrowserMetadataContext = "regular" | "incognito";

/**
 * Routes regular metadata through the profile-scoped persistence boundary and
 * keeps explicitly enabled incognito metadata only in this session instance.
 */
export class BrowserProfilePrivacyBoundary<Metadata> {
  #incognitoEnabled = false;
  #incognitoMetadata: Metadata | null = null;

  public constructor(
    private readonly persistent: BrowserProfileMetadataBoundary<Metadata>,
  ) {}

  public get identity(): BrowserProfileIdentity {
    return this.persistent.identity;
  }

  public get incognitoEnabled(): boolean {
    return this.#incognitoEnabled;
  }

  public enableIncognitoSession(): void {
    this.#incognitoEnabled = true;
  }

  public endIncognitoSession(): void {
    this.#incognitoMetadata = null;
    this.#incognitoEnabled = false;
  }

  public async load(context: BrowserMetadataContext): Promise<Metadata | null> {
    if (context === "regular") return await this.persistent.load();
    this.#requireIncognitoEnabled();
    return this.#incognitoMetadata;
  }

  public async save(
    context: BrowserMetadataContext,
    metadata: Metadata,
  ): Promise<void> {
    if (context === "regular") return await this.persistent.save(metadata);
    this.#requireIncognitoEnabled();
    this.#incognitoMetadata = metadata;
  }

  #requireIncognitoEnabled(): void {
    if (!this.#incognitoEnabled) {
      throw new Error("incognito integration is disabled");
    }
  }
}

export function createBrowserProfileScope(
  identity: BrowserProfileIdentity,
): BrowserProfileScope {
  if (!SUPPORTED_BROWSER_IDS.includes(identity.browser)) {
    throw new Error("unsupported browser identity");
  }
  if (!/^[A-Za-z0-9_-]{16,128}$/u.test(identity.profileId)) {
    throw new Error("invalid browser profile ID");
  }

  return Object.freeze({
    browser: identity.browser,
    profileId: identity.profileId,
    key: JSON.stringify([identity.browser, identity.profileId]),
  });
}
