export function formatLine(
  level: string,
  name: string,
  message: string,
  fields?: Record<string, unknown>,
): string {
  const timestamp = new Date().toISOString();
  const base = `${timestamp} ${level.toUpperCase()} [${name}] ${message}`;
  if (!fields) {
    return base;
  }
  return `${base} ${JSON.stringify(fields)}`;
}
