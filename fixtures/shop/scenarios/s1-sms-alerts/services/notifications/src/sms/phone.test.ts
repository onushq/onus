import { describe, expect, it } from "vitest";
import { countryFromLocale, isE164, maskPhone, normalizePhone } from "./phone";

describe("normalizePhone", () => {
  it("formats a US national number", () => {
    expect(normalizePhone("(415) 555-0123", "US")).toBe("+14155550123");
  });

  it("accepts a US number with a leading country code", () => {
    expect(normalizePhone("1 415 555 0123", "US")).toBe("+14155550123");
  });

  it("drops the UK trunk prefix", () => {
    expect(normalizePhone("07700 900123", "GB")).toBe("+447700900123");
  });

  it("keeps numbers that are already international", () => {
    expect(normalizePhone("+49 151 23456789", "US")).toBe("+4915123456789");
  });

  it("understands the 00 international prefix", () => {
    expect(normalizePhone("0033 6 12 34 56 78", "US")).toBe("+33612345678");
  });

  it("rejects numbers with the wrong length for the country", () => {
    expect(normalizePhone("555-0123", "US")).toBeNull();
  });

  it("rejects letters and other junk", () => {
    expect(normalizePhone("call me maybe", "US")).toBeNull();
    expect(normalizePhone("1-800-FLOWERS", "US")).toBeNull();
  });

  it("rejects a plus sign in the middle of the number", () => {
    expect(normalizePhone("415+5550123", "US")).toBeNull();
  });
});

describe("isE164", () => {
  it("accepts well-formed numbers", () => {
    expect(isE164("+14155550123")).toBe(true);
  });

  it("rejects numbers without a plus or with a leading zero", () => {
    expect(isE164("14155550123")).toBe(false);
    expect(isE164("+04155550123")).toBe(false);
  });
});

describe("countryFromLocale", () => {
  it("reads the region from the locale", () => {
    expect(countryFromLocale("de-DE")).toBe("DE");
  });

  it("falls back to US for unknown or missing regions", () => {
    expect(countryFromLocale("pt-BR")).toBe("US");
    expect(countryFromLocale(undefined)).toBe("US");
  });
});

describe("maskPhone", () => {
  it("only reveals the last four digits", () => {
    expect(maskPhone("+14155550123")).toBe("*******0123");
  });

  it("fully masks very short values", () => {
    expect(maskPhone("123")).toBe("****");
  });
});
