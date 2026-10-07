function renderValue(value: unknown): string {
  if (typeof value === "string") {
    return /\s/.test(value) ? JSON.stringify(value) : value;
  }
  if (value instanceof Date) {
    return value.toISOString();
  }
  return JSON.stringify(value) ?? "undefined";
}

function renderFields(fields: Record<string, unknown>): string {
  return Object.keys(fields)
    .sort()
    .map((key) => `${key}=${renderValue(fields[key])}`)
    .join(" ");
}

export function formatLine(
  level: string,
  name: string,
  message: string,
  fields?: Record<string, unknown>,
): string {
  const timestamp = new Date().toISOString();
  const base = `${timestamp} ${level.toUpperCase()} [${name}] ${message}`;
  if (!fields || Object.keys(fields).length === 0) {
    return base;
  }
  return `${base} ${renderFields(fields)}`;
}
