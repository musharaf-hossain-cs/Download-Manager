import type { DownloadJob } from "../src/protocol.js";

export function jobFixture(overrides: Partial<DownloadJob> = {}): DownloadJob {
  return {
    id: "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
    source_url: "https://example.test/archive.zip",
    final_url: null,
    display_url: "https://example.test/archive.zip",
    filename: "archive.zip",
    destination: "C:\\Downloads",
    state: "queued",
    kind: "file",
    total_bytes: 1_000,
    downloaded_bytes: 0,
    category: "Compressed",
    queue: null,
    priority: "normal",
    source_browser: null,
    source_profile: null,
    retry_count: 0,
    error: null,
    created_at: 1_700_000_000_000,
    updated_at: 1_700_000_000_000,
    completed_at: null,
    ...overrides,
  };
}
