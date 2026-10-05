export interface RetryOptions {
  /** Total attempts including the first one. */
  maxAttempts: number;
  baseDelayMs: number;
  maxDelayMs: number;
  /** Decides whether a failure is worth another attempt. */
  isRetryable: (error: unknown) => boolean;
  /** Injected so tests do not have to wait for real time to pass. */
  sleep?: (ms: number) => Promise<void>;
  /** Injected so tests get deterministic jitter. */
  random?: () => number;
  onRetry?: (attempt: number, delayMs: number, error: unknown) => void;
}

export interface RetryResult<T> {
  value: T;
  attempts: number;
}

export const DEFAULT_RETRY_OPTIONS: Omit<RetryOptions, "isRetryable"> = {
  maxAttempts: 4,
  baseDelayMs: 500,
  maxDelayMs: 8_000,
};

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

/** Exponential backoff with "full jitter": a random delay between 0 and the capped exponential step. */
export function backoffDelay(attempt: number, options: Pick<RetryOptions, "baseDelayMs" | "maxDelayMs" | "random">): number {
  const random = options.random ?? Math.random;
  const exponential = options.baseDelayMs * 2 ** (attempt - 1);
  const capped = Math.min(exponential, options.maxDelayMs);
  return Math.floor(random() * capped);
}

export class RetryExhaustedError extends Error {
  readonly attempts: number;
  readonly lastError: unknown;

  constructor(attempts: number, lastError: unknown) {
    super(`Gave up after ${attempts} attempt(s)`);
    this.name = "RetryExhaustedError";
    this.attempts = attempts;
    this.lastError = lastError;
  }
}

export async function withRetry<T>(
  operation: (attempt: number) => Promise<T>,
  options: RetryOptions,
): Promise<RetryResult<T>> {
  const sleep = options.sleep ?? defaultSleep;
  let lastError: unknown;
  for (let attempt = 1; attempt <= options.maxAttempts; attempt += 1) {
    try {
      const value = await operation(attempt);
      return { value, attempts: attempt };
    } catch (error: unknown) {
      lastError = error;
      if (!options.isRetryable(error) || attempt === options.maxAttempts) {
        throw new RetryExhaustedError(attempt, error);
      }
      const delayMs = backoffDelay(attempt, options);
      options.onRetry?.(attempt, delayMs, error);
      await sleep(delayMs);
    }
  }
  throw new RetryExhaustedError(options.maxAttempts, lastError);
}
