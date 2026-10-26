/** Internal HTTP(S) URL normalizer shared by browser capture adapters. */
export function normalizeHttpUrl(
  value: string | undefined,
  base?: string,
): string | null {
  if (value === undefined || value.length > 8_192) return null;
  try {
    const url = new URL(value, base);
    if (
      (url.protocol !== "http:" && url.protocol !== "https:") ||
      url.hostname.length === 0
    ) {
      return null;
    }
    return url.toString();
  } catch {
    return null;
  }
}
