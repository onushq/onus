import { afterEach, describe, expect, it, vi } from "vitest";

const create = vi.fn();

vi.mock("@acme/sms", () => ({
  AcmeSmsClient: vi.fn(() => ({ messages: { create } })),
}));

import { getSmsClient, setSmsClientForTesting, toDeliveryError } from "./client";

describe("toDeliveryError", () => {
  it("marks throttling and server errors as retryable", () => {
    expect(toDeliveryError({ statusCode: 429, message: "slow down" }).retryable).toBe(true);
    expect(toDeliveryError({ statusCode: 503, message: "unavailable" }).retryable).toBe(true);
  });

  it("marks client errors as permanent", () => {
    const error = toDeliveryError({ statusCode: 400, message: "bad request" });
    expect(error.retryable).toBe(false);
    expect(error.statusCode).toBe(400);
  });

  it("never retries numbers the provider says are unreachable", () => {
    const error = toDeliveryError({ statusCode: 503, code: "landline", message: "cannot text" });
    expect(error.retryable).toBe(false);
    expect(error.message).toBe("cannot text (landline)");
  });
});

describe("getSmsClient", () => {
  afterEach(() => {
    setSmsClientForTesting(undefined);
    vi.unstubAllEnvs();
    create.mockReset();
  });

  it("refuses to start without an API key", () => {
    vi.stubEnv("ACME_SMS_API_KEY", "");
    expect(() => getSmsClient()).toThrow("ACME_SMS_API_KEY");
  });

  it("sends through the provider and maps the response", async () => {
    vi.stubEnv("ACME_SMS_API_KEY", "test-key");
    create.mockResolvedValue({ id: "msg_1", segments: 1 });

    const result = await getSmsClient().send({ to: "+14155550123", body: "hi", correlationId: "c1" });

    expect(result).toEqual({ providerMessageId: "msg_1", segments: 1 });
    expect(create).toHaveBeenCalledWith({ to: "+14155550123", from: "SHOP", body: "hi", reference: "c1" });
  });

  it("wraps provider failures", async () => {
    vi.stubEnv("ACME_SMS_API_KEY", "test-key");
    create.mockRejectedValue({ statusCode: 502, message: "bad gateway" });

    await expect(
      getSmsClient().send({ to: "+14155550123", body: "hi", correlationId: "c1" }),
    ).rejects.toMatchObject({ name: "SmsDeliveryError", retryable: true });
  });
});
