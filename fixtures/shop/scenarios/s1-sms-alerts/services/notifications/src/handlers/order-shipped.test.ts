import { beforeEach, describe, expect, it, vi } from "vitest";

const checkSmsEligibility = vi.fn();
const deliverSms = vi.fn();
const recordDelivery = vi.fn();

vi.mock("../sms/eligibility", () => ({ checkSmsEligibility }));
vi.mock("../sms/send", () => ({ deliverSms }));
vi.mock("../sms/delivery-log", () => ({ recordDelivery }));

import { onOrderShipped } from "./order-shipped";

const event = {
  orderId: "ord_abc123def456",
  customerId: "cus_1",
  carrier: "ups",
  shippedAt: new Date("2026-01-15T17:00:00Z"),
};

describe("onOrderShipped", () => {
  beforeEach(() => {
    checkSmsEligibility.mockReset();
    deliverSms.mockReset();
    recordDelivery.mockReset();
  });

  it("texts eligible customers", async () => {
    checkSmsEligibility.mockResolvedValue({ eligible: true, phone: "+14155550123", locale: "en-US" });
    deliverSms.mockResolvedValue("sent");

    await onOrderShipped(event);

    expect(deliverSms).toHaveBeenCalledWith({
      correlationId: "order-shipped:ord_abc123def456",
      to: "+14155550123",
      body: "Your order ord_abc123def456 has shipped with UPS. Reply STOP to opt out.",
      timeZone: "America/New_York",
    });
  });

  it("skips customers who are not eligible", async () => {
    checkSmsEligibility.mockResolvedValue({ eligible: false, reason: "sms_opt_out" });

    await onOrderShipped(event);

    expect(deliverSms).not.toHaveBeenCalled();
    expect(recordDelivery).toHaveBeenCalledWith({
      correlationId: "order-shipped:ord_abc123def456",
      outcome: "ineligible",
      reason: "sms_opt_out",
    });
  });

  it("logs unusable phone numbers separately", async () => {
    checkSmsEligibility.mockResolvedValue({ eligible: false, reason: "invalid_phone" });

    await onOrderShipped(event);

    expect(recordDelivery).toHaveBeenCalledWith(expect.objectContaining({ outcome: "invalid_phone" }));
  });

  it("never throws back into the shipping flow", async () => {
    checkSmsEligibility.mockRejectedValue(new Error("No preferences for user cus_1"));

    await expect(onOrderShipped(event)).resolves.toBeUndefined();
    expect(deliverSms).not.toHaveBeenCalled();
  });
});
