import { getSmsClient } from "./client";
import type { SmsClient } from "./client";
import { recordDelivery } from "./delivery-log";
import { nextSendTime } from "./quiet-hours";
import { createSmsRateLimiter } from "./rate-limit";
import type { SmsRateLimiter } from "./rate-limit";
import { DEFAULT_RETRY_OPTIONS, RetryExhaustedError, withRetry } from "./retry";
import type { RetryOptions } from "./retry";
import { SmsDeliveryError } from "./types";
import type { DeliveryOutcome, E164Phone } from "./types";

export interface SmsRequest {
  correlationId: string;
  to: E164Phone;
  body: string;
  /** IANA time zone of the recipient, used for quiet hours. */
  timeZone: string;
}

export interface SendDependencies {
  client: () => SmsClient;
  rateLimiter: SmsRateLimiter;
  now: () => Date;
  /** Runs a task later. The default uses a timer, so deferred texts do not survive a restart. */
  schedule: (at: Date, task: () => void) => void;
  retry: Omit<RetryOptions, "isRetryable">;
}

const sharedRateLimiter = createSmsRateLimiter();

function scheduleWithTimer(at: Date, task: () => void): void {
  const delayMs = Math.max(0, at.getTime() - Date.now());
  setTimeout(task, delayMs);
}

export function defaultSendDependencies(): SendDependencies {
  return {
    client: getSmsClient,
    rateLimiter: sharedRateLimiter,
    now: () => new Date(),
    schedule: scheduleWithTimer,
    retry: DEFAULT_RETRY_OPTIONS,
  };
}

function isRetryableSmsError(error: unknown): boolean {
  return error instanceof SmsDeliveryError && error.retryable;
}

function describeFailure(error: unknown): string {
  const cause = error instanceof RetryExhaustedError ? error.lastError : error;
  return cause instanceof Error ? cause.message : String(cause);
}

/**
 * Sends one SMS, honouring quiet hours, rate limits and transient provider failures.
 * Never throws for delivery problems: the outcome is returned and logged instead.
 */
export async function deliverSms(
  request: SmsRequest,
  deps: SendDependencies = defaultSendDependencies(),
): Promise<DeliveryOutcome> {
  const now = deps.now();
  const resumeAt = nextSendTime(now, request.timeZone);
  if (resumeAt !== null) {
    deps.schedule(resumeAt, () => {
      void deliverSms(request, deps);
    });
    recordDelivery({
      correlationId: request.correlationId,
      outcome: "deferred",
      to: request.to,
      reason: `resume at ${resumeAt.toISOString()}`,
    });
    return "deferred";
  }

  if (!deps.rateLimiter.allow(request.to, now.getTime())) {
    recordDelivery({ correlationId: request.correlationId, outcome: "rate_limited", to: request.to });
    return "rate_limited";
  }

  try {
    const client = deps.client();
    const { value, attempts } = await withRetry(
      () => client.send({ to: request.to, body: request.body, correlationId: request.correlationId }),
      { ...deps.retry, isRetryable: isRetryableSmsError },
    );
    recordDelivery({
      correlationId: request.correlationId,
      outcome: "sent",
      to: request.to,
      attempts,
      providerMessageId: value.providerMessageId,
    });
    return "sent";
  } catch (error: unknown) {
    recordDelivery({
      correlationId: request.correlationId,
      outcome: "failed",
      to: request.to,
      attempts: error instanceof RetryExhaustedError ? error.attempts : undefined,
      reason: describeFailure(error),
    });
    return "failed";
  }
}
