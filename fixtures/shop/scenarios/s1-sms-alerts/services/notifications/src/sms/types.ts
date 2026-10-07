/** ISO 3166-1 alpha-2 country codes we currently ship to. */
export type CountryCode = "US" | "CA" | "GB" | "DE" | "FR" | "IE" | "NL";

/** A phone number in E.164 form, e.g. "+14155550123". */
export type E164Phone = string;

export interface SmsMessage {
  to: E164Phone;
  body: string;
  /** Used to group log lines for one notification across retries. */
  correlationId: string;
}

export interface SmsSendResult {
  providerMessageId: string;
  segments: number;
}

export type DeliveryOutcome =
  | "sent"
  | "deferred"
  | "rate_limited"
  | "ineligible"
  | "invalid_phone"
  | "failed";

export interface DeliveryLogEntry {
  correlationId: string;
  outcome: DeliveryOutcome;
  to?: E164Phone;
  attempts?: number;
  providerMessageId?: string;
  reason?: string;
}

/** Raised by the SMS client wrapper so callers never see provider errors directly. */
export class SmsDeliveryError extends Error {
  readonly retryable: boolean;
  readonly statusCode: number | undefined;

  constructor(message: string, retryable: boolean, statusCode?: number) {
    super(message);
    this.name = "SmsDeliveryError";
    this.retryable = retryable;
    this.statusCode = statusCode;
  }
}
