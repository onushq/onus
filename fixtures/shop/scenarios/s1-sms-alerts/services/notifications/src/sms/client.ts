import { AcmeSmsClient } from "@acme/sms";
import { SmsDeliveryError } from "./types";
import type { SmsMessage, SmsSendResult } from "./types";

/** Narrow surface the rest of the SMS code depends on; easy to fake in tests. */
export interface SmsClient {
  send(message: SmsMessage): Promise<SmsSendResult>;
}

interface ProviderErrorShape {
  message?: unknown;
  statusCode?: unknown;
  code?: unknown;
}

const DEFAULT_SENDER_ID = "SHOP";

/** Provider status codes that are worth another attempt. */
const RETRYABLE_STATUS_CODES = new Set([408, 429, 500, 502, 503, 504]);

/** Provider error codes for numbers that will never accept a message. */
const PERMANENT_ERROR_CODES = new Set(["unreachable_number", "landline", "blocked_recipient"]);

function readProviderError(error: unknown): ProviderErrorShape {
  if (typeof error === "object" && error !== null) {
    return error as ProviderErrorShape;
  }
  return { message: String(error) };
}

export function toDeliveryError(error: unknown): SmsDeliveryError {
  const shape = readProviderError(error);
  const statusCode = typeof shape.statusCode === "number" ? shape.statusCode : undefined;
  const code = typeof shape.code === "string" ? shape.code : undefined;
  const message = typeof shape.message === "string" ? shape.message : "SMS provider error";
  if (code !== undefined && PERMANENT_ERROR_CODES.has(code)) {
    return new SmsDeliveryError(`${message} (${code})`, false, statusCode);
  }
  const retryable = statusCode === undefined || RETRYABLE_STATUS_CODES.has(statusCode);
  return new SmsDeliveryError(message, retryable, statusCode);
}

class AcmeSmsAdapter implements SmsClient {
  constructor(
    private readonly provider: AcmeSmsClient,
    private readonly senderId: string,
  ) {}

  async send(message: SmsMessage): Promise<SmsSendResult> {
    try {
      const response = await this.provider.messages.create({
        to: message.to,
        from: this.senderId,
        body: message.body,
        reference: message.correlationId,
      });
      return { providerMessageId: response.id, segments: response.segments };
    } catch (error: unknown) {
      throw toDeliveryError(error);
    }
  }
}

let shared: SmsClient | undefined;

/** Lazily builds the process-wide client so importing this module never needs credentials. */
export function getSmsClient(): SmsClient {
  if (shared) {
    return shared;
  }
  const apiKey = process.env.ACME_SMS_API_KEY;
  if (!apiKey) {
    throw new Error("ACME_SMS_API_KEY is not set");
  }
  const senderId = process.env.ACME_SMS_SENDER_ID ?? DEFAULT_SENDER_ID;
  shared = new AcmeSmsAdapter(new AcmeSmsClient({ apiKey }), senderId);
  return shared;
}

/** Test seam: swap the shared client for a fake (or reset it with undefined). */
export function setSmsClientForTesting(client: SmsClient | undefined): void {
  shared = client;
}
