/** Canonical runtime object guard for data parsed at the store/import boundary. */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
