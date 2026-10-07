export interface RateLimitRule {
  /** Maximum number of sends allowed inside the window. */
  limit: number;
  windowMs: number;
}

/** At most 3 texts per recipient per hour, no matter how many orders ship. */
export const PER_RECIPIENT_RULE: RateLimitRule = { limit: 3, windowMs: 60 * 60 * 1000 };
/** Stay well under the provider's account-wide throughput cap. */
export const GLOBAL_RULE: RateLimitRule = { limit: 20, windowMs: 1000 };

/**
 * Sliding-window limiter kept in memory. Good enough for a single notifications
 * worker; each instance enforces its own budget.
 */
export class SlidingWindowRateLimiter {
  private readonly hits = new Map<string, number[]>();

  constructor(private readonly rule: RateLimitRule) {}

  private prune(key: string, now: number): number[] {
    const windowStart = now - this.rule.windowMs;
    const recent = (this.hits.get(key) ?? []).filter((timestamp) => timestamp > windowStart);
    if (recent.length === 0) {
      this.hits.delete(key);
    } else {
      this.hits.set(key, recent);
    }
    return recent;
  }

  tryAcquire(key: string, now: number = Date.now()): boolean {
    const recent = this.prune(key, now);
    if (recent.length >= this.rule.limit) {
      return false;
    }
    this.hits.set(key, [...recent, now]);
    return true;
  }

  remaining(key: string, now: number = Date.now()): number {
    return Math.max(0, this.rule.limit - this.prune(key, now).length);
  }

  reset(): void {
    this.hits.clear();
  }
}

const GLOBAL_KEY = "*";

export interface SmsRateLimiter {
  allow(recipient: string, now?: number): boolean;
  reset(): void;
}

export function createSmsRateLimiter(
  perRecipient: RateLimitRule = PER_RECIPIENT_RULE,
  global: RateLimitRule = GLOBAL_RULE,
): SmsRateLimiter {
  const recipientLimiter = new SlidingWindowRateLimiter(perRecipient);
  const globalLimiter = new SlidingWindowRateLimiter(global);
  return {
    allow(recipient: string, now: number = Date.now()): boolean {
      if (recipientLimiter.remaining(recipient, now) === 0) {
        return false;
      }
      if (!globalLimiter.tryAcquire(GLOBAL_KEY, now)) {
        return false;
      }
      return recipientLimiter.tryAcquire(recipient, now);
    },
    reset(): void {
      recipientLimiter.reset();
      globalLimiter.reset();
    },
  };
}
