import type { CountryCode, E164Phone } from "./types";

interface CountryRule {
  dialCode: string;
  /** Length of the national significant number, without trunk prefix. */
  nationalLengths: number[];
  /** Domestic trunk prefix that is dropped when dialing internationally. */
  trunkPrefix?: string;
}

const COUNTRY_RULES: Record<CountryCode, CountryRule> = {
  US: { dialCode: "1", nationalLengths: [10] },
  CA: { dialCode: "1", nationalLengths: [10] },
  GB: { dialCode: "44", nationalLengths: [10], trunkPrefix: "0" },
  DE: { dialCode: "49", nationalLengths: [10, 11], trunkPrefix: "0" },
  FR: { dialCode: "33", nationalLengths: [9], trunkPrefix: "0" },
  IE: { dialCode: "353", nationalLengths: [9], trunkPrefix: "0" },
  NL: { dialCode: "31", nationalLengths: [9], trunkPrefix: "0" },
};

const E164_PATTERN = /^\+[1-9]\d{6,14}$/;
const ALLOWED_CHARACTERS = /^[\d\s().+-]+$/;

export function isE164(value: string): value is E164Phone {
  return E164_PATTERN.test(value);
}

function stripFormatting(raw: string): string {
  return raw.replace(/[\s().-]/g, "");
}

function fromInternational(digits: string): E164Phone | null {
  const candidate = `+${digits}`;
  return isE164(candidate) ? candidate : null;
}

function fromNational(digits: string, rule: CountryRule): E164Phone | null {
  let national = digits;
  if (rule.trunkPrefix !== undefined && national.startsWith(rule.trunkPrefix)) {
    national = national.slice(rule.trunkPrefix.length);
  }
  if (rule.dialCode === "1" && national.length === 11 && national.startsWith("1")) {
    national = national.slice(1);
  }
  if (!rule.nationalLengths.includes(national.length)) {
    return null;
  }
  return fromInternational(`${rule.dialCode}${national}`);
}

/**
 * Normalizes user-entered phone numbers to E.164.
 * Returns null for anything we cannot confidently interpret; callers must not guess.
 */
export function normalizePhone(raw: string, defaultCountry: CountryCode): E164Phone | null {
  const trimmed = raw.trim();
  if (trimmed.length === 0 || !ALLOWED_CHARACTERS.test(trimmed)) {
    return null;
  }
  const compact = stripFormatting(trimmed);
  if (compact.indexOf("+") > 0) {
    return null;
  }
  if (compact.startsWith("+")) {
    return fromInternational(compact.slice(1));
  }
  if (compact.startsWith("00")) {
    return fromInternational(compact.slice(2));
  }
  return fromNational(compact, COUNTRY_RULES[defaultCountry]);
}

/** Best-effort country guess from a BCP 47 locale like "en-GB". */
export function countryFromLocale(locale: string | undefined): CountryCode {
  const region = locale?.split("-")[1]?.toUpperCase();
  if (region !== undefined && region in COUNTRY_RULES) {
    return region as CountryCode;
  }
  return "US";
}

/** Keeps only the last four digits so phone numbers never reach the logs in full. */
export function maskPhone(phone: string): string {
  const digits = phone.replace(/\D/g, "");
  if (digits.length <= 4) {
    return "****";
  }
  return `${"*".repeat(digits.length - 4)}${digits.slice(-4)}`;
}
