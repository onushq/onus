import { describe, expect, it } from "vitest";
import { createSmsRateLimiter, SlidingWindowRateLimiter } from "./rate-limit";

const HOUR = 60 * 60 * 1000;

describe("SlidingWindowRateLimiter", () => {
  it("allows requests up to the limit", () => {
    const limiter = new SlidingWindowRateLimiter({ limit: 2, windowMs: 1_000 });
    expect(limiter.tryAcquire("a", 0)).toBe(true);
    expect(limiter.tryAcquire("a", 10)).toBe(true);
    expect(limiter.tryAcquire("a", 20)).toBe(false);
  });

  it("frees capacity as the window slides", () => {
    const limiter = new SlidingWindowRateLimiter({ limit: 1, windowMs: 1_000 });
    expect(limiter.tryAcquire("a", 0)).toBe(true);
    expect(limiter.tryAcquire("a", 999)).toBe(false);
    expect(limiter.tryAcquire("a", 1_001)).toBe(true);
  });

  it("reports remaining capacity without consuming it", () => {
    const limiter = new SlidingWindowRateLimiter({ limit: 3, windowMs: 1_000 });
    limiter.tryAcquire("a", 0);
    expect(limiter.remaining("a", 1)).toBe(2);
    expect(limiter.remaining("a", 1)).toBe(2);
  });
});

describe("createSmsRateLimiter", () => {
  it("limits each recipient to three texts per hour", () => {
    const limiter = createSmsRateLimiter();
    const phone = "+14155550123";
    expect(limiter.allow(phone, 0)).toBe(true);
    expect(limiter.allow(phone, 5_000)).toBe(true);
    expect(limiter.allow(phone, 10_000)).toBe(true);
    expect(limiter.allow(phone, 15_000)).toBe(false);
    expect(limiter.allow(phone, HOUR + 1)).toBe(true);
  });

  it("enforces the global budget across recipients", () => {
    const limiter = createSmsRateLimiter({ limit: 10, windowMs: HOUR }, { limit: 2, windowMs: 1_000 });
    expect(limiter.allow("+14155550001", 0)).toBe(true);
    expect(limiter.allow("+14155550002", 0)).toBe(true);
    expect(limiter.allow("+14155550003", 0)).toBe(false);
  });

  it("forgets everything on reset", () => {
    const limiter = createSmsRateLimiter({ limit: 1, windowMs: HOUR });
    limiter.allow("+14155550123", 0);
    limiter.reset();
    expect(limiter.allow("+14155550123", 1)).toBe(true);
  });
});
