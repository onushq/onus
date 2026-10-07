import { describe, expect, it, vi } from "vitest";
import { backoffDelay, RetryExhaustedError, withRetry } from "./retry";
import type { RetryOptions } from "./retry";

class Transient extends Error {}
class Permanent extends Error {}

function options(overrides: Partial<RetryOptions> = {}): RetryOptions {
  return {
    maxAttempts: 3,
    baseDelayMs: 100,
    maxDelayMs: 1_000,
    isRetryable: (error: unknown) => error instanceof Transient,
    sleep: vi.fn(async () => undefined),
    random: () => 1,
    ...overrides,
  };
}

describe("withRetry", () => {
  it("returns the first successful result", async () => {
    const result = await withRetry(async () => "ok", options());
    expect(result).toEqual({ value: "ok", attempts: 1 });
  });

  it("retries transient failures and reports the attempt count", async () => {
    const operation = vi
      .fn<(attempt: number) => Promise<string>>()
      .mockRejectedValueOnce(new Transient("blip"))
      .mockResolvedValueOnce("ok");

    const result = await withRetry(operation, options());

    expect(result.attempts).toBe(2);
    expect(operation).toHaveBeenCalledTimes(2);
  });

  it("does not retry permanent failures", async () => {
    const operation = vi.fn(async () => {
      throw new Permanent("no");
    });

    await expect(withRetry(operation, options())).rejects.toBeInstanceOf(RetryExhaustedError);
    expect(operation).toHaveBeenCalledTimes(1);
  });

  it("gives up after maxAttempts", async () => {
    const sleep = vi.fn(async () => undefined);
    const operation = vi.fn(async () => {
      throw new Transient("still down");
    });

    const failure = await withRetry(operation, options({ sleep })).catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(RetryExhaustedError);
    expect((failure as RetryExhaustedError).attempts).toBe(3);
    expect(sleep).toHaveBeenCalledTimes(2);
  });

  it("tells the caller about each retry", async () => {
    const onRetry = vi.fn();
    const operation = vi
      .fn<(attempt: number) => Promise<string>>()
      .mockRejectedValueOnce(new Transient("blip"))
      .mockResolvedValueOnce("ok");

    await withRetry(operation, options({ onRetry }));

    expect(onRetry).toHaveBeenCalledWith(1, 100, expect.any(Transient));
  });
});

describe("backoffDelay", () => {
  it("doubles each attempt", () => {
    const opts = { baseDelayMs: 100, maxDelayMs: 10_000, random: () => 1 };
    expect([1, 2, 3, 4].map((attempt) => backoffDelay(attempt, opts))).toEqual([100, 200, 400, 800]);
  });

  it("is capped at maxDelayMs", () => {
    expect(backoffDelay(10, { baseDelayMs: 100, maxDelayMs: 1_000, random: () => 1 })).toBe(1_000);
  });
});
