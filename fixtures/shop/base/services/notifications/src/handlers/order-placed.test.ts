import { beforeEach, describe, expect, it, vi } from "vitest";

const getPreferences = vi.fn();
const sendEmail = vi.fn();

vi.mock("@shop/user-preferences", () => ({ getPreferences }));
vi.mock("../email/sendgrid", () => ({ sendEmail }));

import { onOrderPlaced } from "./order-placed";

const event = {
  orderId: "ord_abc123def456",
  customerId: "cus_1",
  subtotalCents: 4200,
  totalCents: 4200,
  currency: "USD",
};

describe("onOrderPlaced", () => {
  beforeEach(() => {
    getPreferences.mockReset();
    sendEmail.mockReset();
  });

  it("emails the customer a confirmation", async () => {
    getPreferences.mockResolvedValue({
      userId: "cus_1",
      email: "ada@example.com",
      emailOptOut: false,
      smsOptOut: false,
    });

    await onOrderPlaced(event);

    expect(sendEmail).toHaveBeenCalledOnce();
    expect(sendEmail.mock.calls[0][0]).toBe("ada@example.com");
    expect(sendEmail.mock.calls[0][2]).toContain("ord_abc123def456");
  });

  it("respects the email opt-out", async () => {
    getPreferences.mockResolvedValue({
      userId: "cus_1",
      email: "ada@example.com",
      emailOptOut: true,
      smsOptOut: false,
    });

    await onOrderPlaced(event);

    expect(sendEmail).not.toHaveBeenCalled();
  });
});
