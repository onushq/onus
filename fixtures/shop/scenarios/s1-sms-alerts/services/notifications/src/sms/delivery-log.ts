import { createLogger } from "@shop/logger";
import { maskPhone } from "./phone";
import type { DeliveryLogEntry, DeliveryOutcome } from "./types";

const log = createLogger("notifications.sms");

type Level = "info" | "warn" | "error";

const LEVEL_BY_OUTCOME: Record<DeliveryOutcome, Level> = {
  sent: "info",
  deferred: "info",
  ineligible: "info",
  rate_limited: "warn",
  invalid_phone: "warn",
  failed: "error",
};

const MESSAGE_BY_OUTCOME: Record<DeliveryOutcome, string> = {
  sent: "sms sent",
  deferred: "sms deferred until quiet hours end",
  ineligible: "sms skipped, recipient not eligible",
  rate_limited: "sms dropped by rate limiter",
  invalid_phone: "sms skipped, phone number could not be normalized",
  failed: "sms delivery failed",
};

/** Builds the structured fields for a delivery log line. Phone numbers are always masked. */
export function deliveryFields(entry: DeliveryLogEntry): Record<string, unknown> {
  const fields: Record<string, unknown> = {
    correlationId: entry.correlationId,
    outcome: entry.outcome,
  };
  if (entry.to !== undefined) {
    fields.to = maskPhone(entry.to);
  }
  if (entry.attempts !== undefined) {
    fields.attempts = entry.attempts;
  }
  if (entry.providerMessageId !== undefined) {
    fields.providerMessageId = entry.providerMessageId;
  }
  if (entry.reason !== undefined) {
    fields.reason = entry.reason;
  }
  return fields;
}

export function recordDelivery(entry: DeliveryLogEntry): void {
  const level = LEVEL_BY_OUTCOME[entry.outcome];
  log[level](MESSAGE_BY_OUTCOME[entry.outcome], deliveryFields(entry));
}
