import { getPreferences } from "@shop/user-preferences";
import type { UserPreferences } from "@shop/user-preferences";
import { countryFromLocale, normalizePhone } from "./phone";
import type { E164Phone } from "./types";

export type IneligibleReason = "no_phone" | "phone_unverified" | "sms_opt_out" | "invalid_phone";

export type SmsEligibility =
  | { eligible: true; phone: E164Phone; locale?: string }
  | { eligible: false; reason: IneligibleReason };

function ineligible(reason: IneligibleReason): SmsEligibility {
  return { eligible: false, reason };
}

/**
 * We only text customers who gave us a phone number, verified it, and have not
 * opted out of SMS. An unverified number may belong to someone else entirely.
 */
export function evaluateEligibility(prefs: UserPreferences): SmsEligibility {
  if (!prefs.phone) {
    return ineligible("no_phone");
  }
  const verified = prefs.phoneVerified === true;
  if (!verified) {
    return ineligible("phone_unverified");
  }
  const optedIn = !prefs.smsOptOut;
  if (!optedIn) {
    return ineligible("sms_opt_out");
  }
  const phone = normalizePhone(prefs.phone, countryFromLocale(prefs.locale));
  if (phone === null) {
    return ineligible("invalid_phone");
  }
  return { eligible: true, phone, locale: prefs.locale };
}

export async function checkSmsEligibility(userId: string): Promise<SmsEligibility> {
  const prefs = await getPreferences(userId);
  return evaluateEligibility(prefs);
}
