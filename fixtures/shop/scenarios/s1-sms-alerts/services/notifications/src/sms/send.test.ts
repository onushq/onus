import { beforeEach, describe, expect, it, vi } from "vitest";

const recordDelivery = vi.fn();

vi.mock("./delivery-log", () => ({ recordDelivery }));

import type { SmsClient } from "./client";
import { createSmsRateLimiter } from "./rate-limit";
import { deliverSms } from "./send";
import type { SendDependencies, SmsRequest } from "./send";
import { SmsDeliveryError } from "./types";

const request: SmsRequest = {
  correlationId: "order-shipped:ord_1",
  to: "+14155550123",
  body: "Your order ord_1 has shipped with UPS. Reply STOP to opt out.",
  timeZone: "America/New_York",
};

// 12:00 in New York.
const MIDDAY = new Date("2026-01-15T17:00:00Z");
// 23:00 in New York.
const LATE = new Date("2026-01-16T04:00:00Z");

function deps(send: SmsClient["send"], now: Date = MIDDAY): SendDependencies {
  return {
    client: () => ({ send }),
    rateLimiter: createSmsRateLimiter(),
    now: () => now,
    schedule: vi.fn(),
    retry: { maxAttempts: 3, baseDelayMs: 1, maxDelayMs: 1, sleep: async () => undefined },
  };
}

describe("deliverSms", () => {
  beforeEach(() => {
    recordDelivery.mockReset();
  });

  it("sends immediately during the day", async () => {
    const send = vi.fn(async () => ({ providerMessageId: "msg_1", segments: 1 }));

    const outcome = await deliverSms(request, deps(send));

    expect(outcome).toBe("sent");
    expect(send).toHaveBeenCalledWith({ to: request.to, body: request.body, correlationId: request.correlationId });
    expect(recordDelivery).toHaveBeenCalledWith(
      expect.objectContaining({ outcome: "sent", attempts: 1, providerMessageId: "msg_1" }),
    );
  });

  it("defers texts during quiet hours instead of sending", async () => {
    const send = vi.fn();
    const dependencies = deps(send, LATE);

    const outcome = await deliverSms(request, dependencies);

    expect(outcome).toBe("deferred");
    expect(send).not.toHaveBeenCalled();
    expect(dependencies.schedule).toHaveBeenCalledWith(new Date("2026-01-16T13:00:00Z"), expect.any(Function));
  });

  it("drops texts over the rate limit", async () => {
    const send = vi.fn(async () => ({ providerMessageId: "msg", segments: 1 }));
    const dependencies = deps(send);

    for (let i = 0; i < 3; i += 1) {
      await deliverSms(request, dependencies);
    }
    const outcome = await deliverSms(request, dependencies);

    expect(outcome).toBe("rate_limited");
    expect(send).toHaveBeenCalledTimes(3);
  });

  it("retries transient provider errors", async () => {
    const send = vi
      .fn<SmsClient["send"]>()
      .mockRejectedValueOnce(new SmsDeliveryError("busy", true, 503))
      .mockResolvedValueOnce({ providerMessageId: "msg_2", segments: 1 });

    const outcome = await deliverSms(request, deps(send));

    expect(outcome).toBe("sent");
    expect(recordDelivery).toHaveBeenCalledWith(expect.objectContaining({ attempts: 2 }));
  });

  it("reports failure without throwing when the provider rejects the number", async () => {
    const send = vi.fn(async () => {
      throw new SmsDeliveryError("cannot text (landline)", false, 400);
    });

    const outcome = await deliverSms(request, deps(send));

    expect(outcome).toBe("failed");
    expect(send).toHaveBeenCalledTimes(1);
    expect(recordDelivery).toHaveBeenCalledWith(
      expect.objectContaining({ outcome: "failed", attempts: 1, reason: "cannot text (landline)" }),
    );
  });
});
