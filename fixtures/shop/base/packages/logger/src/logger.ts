import { formatLine } from "./format";

export interface Logger {
  info(message: string, fields?: Record<string, unknown>): void;
  warn(message: string, fields?: Record<string, unknown>): void;
  error(message: string, fields?: Record<string, unknown>): void;
}

export function createLogger(name: string): Logger {
  return {
    info(message: string, fields?: Record<string, unknown>): void {
      console.log(formatLine("info", name, message, fields));
    },
    warn(message: string, fields?: Record<string, unknown>): void {
      console.warn(formatLine("warn", name, message, fields));
    },
    error(message: string, fields?: Record<string, unknown>): void {
      console.error(formatLine("error", name, message, fields));
    },
  };
}
