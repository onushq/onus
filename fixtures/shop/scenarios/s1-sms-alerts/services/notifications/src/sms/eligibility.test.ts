import { describe, expect, it, vi } from "vitest";
import type { UserPreferences } from "@shop/user-preferences";

const getPreferences = vi.fn();

vi.mock("@shop/user-preferences", () => ({ getPreferences }));

import { checkSmsEligibility, evaluateEligibility } from "./eligibility";

function prefs(overrides: Partial<UserPreferences> = {}): UserPreferences {
  return {
    userId: "cus_1",
    email: "ada@example.com",
    phone: "(415) 555-0123",
    phoneVerified: true,
    emailOptOut: false,
    smsOptOut: false,
    locale: "en-US",
    ...overrides,
  };
}

describe("evaluateEligibility", () => {
  it("accepts a verified, opted-in phone and normalizes it", () => {
    expect(evaluateEligibility(prefs())).toEqual({
      eligible: true,
      phone: "+14155550123",
      locale: "en-US",
    });
  });

  it("requires a phone number", () => {
    expect(evaluateEligibility(prefs({ phone: undefined }))).toEqual({ eligible: false, reason: "no_phone" });
  });

  it("requires the phone number to be verified", () => {
    expect(evaluateEligibility(prefs({ phoneVerified: false }))).toEqual({
      eligible: false,
      reason: "phone_unverified",
    });
    expect(evaluateEligibility(prefs({ phoneVerified: undefined }))).toEqual({
      eligible: false,
      reason: "phone_unverified",
    });
  });

  it("respects the SMS opt-out", () => {
    expect(evaluateEligibility(prefs({ smsOptOut: true }))).toEqual({ eligible: false, reason: "sms_opt_out" });
  });

  it("reports numbers that cannot be normalized", () => {
    expect(evaluateEligibility(prefs({ phone: "12345" }))).toEqual({ eligible: false, reason: "invalid_phone" });
  });

  it("uses the locale to interpret national numbers", () => {
    const result = evaluateEligibility(prefs({ phone: "07700 900123", locale: "en-GB" }));
    expect(result).toEqual({ eligible: true, phone: "+447700900123", locale: "en-GB" });
  });
});

describe("checkSmsEligibility", () => {
  it("loads preferences for the user", async () => {
    getPreferences.mockResolvedValue(prefs({ smsOptOut: true }));

    const result = await checkSmsEligibility("cus_1");

    expect(getPreferences).toHaveBeenCalledWith("cus_1");
    expect(result.eligible).toBe(false);
  });
});
