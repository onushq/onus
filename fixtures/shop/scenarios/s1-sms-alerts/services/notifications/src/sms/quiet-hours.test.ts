import { describe, expect, it } from "vitest";
import { isQuietTime, nextSendTime, timeZoneForLocale } from "./quiet-hours";

const NEW_YORK = "America/New_York";

describe("isQuietTime", () => {
  it("is quiet late in the evening", () => {
    // 22:30 in New York (EST, UTC-5).
    expect(isQuietTime(new Date("2026-01-15T03:30:00Z"), NEW_YORK)).toBe(true);
  });

  it("is quiet early in the morning", () => {
    // 06:00 in New York.
    expect(isQuietTime(new Date("2026-01-15T11:00:00Z"), NEW_YORK)).toBe(true);
  });

  it("is not quiet during the day", () => {
    // 12:00 in New York.
    expect(isQuietTime(new Date("2026-01-15T17:00:00Z"), NEW_YORK)).toBe(false);
  });

  it("treats the end hour as the first allowed hour", () => {
    // 08:00 in New York.
    expect(isQuietTime(new Date("2026-01-15T13:00:00Z"), NEW_YORK)).toBe(false);
  });

  it("supports windows that do not cross midnight", () => {
    const lunch = { startHour: 12, endHour: 13 };
    expect(isQuietTime(new Date("2026-01-15T17:30:00Z"), NEW_YORK, lunch)).toBe(true);
    expect(isQuietTime(new Date("2026-01-15T18:30:00Z"), NEW_YORK, lunch)).toBe(false);
  });
});

describe("nextSendTime", () => {
  it("returns null outside quiet hours", () => {
    expect(nextSendTime(new Date("2026-01-15T17:00:00Z"), NEW_YORK)).toBeNull();
  });

  it("waits until 08:00 local time across midnight", () => {
    // 22:30 New York -> 08:00 next morning is 9.5 hours later.
    const resume = nextSendTime(new Date("2026-01-15T03:30:00Z"), NEW_YORK);
    expect(resume?.toISOString()).toBe("2026-01-15T13:00:00.000Z");
  });

  it("waits until 08:00 local time on the same morning", () => {
    // 06:15 London (GMT) -> 08:00 London.
    const resume = nextSendTime(new Date("2026-01-15T06:15:00Z"), "Europe/London");
    expect(resume?.toISOString()).toBe("2026-01-15T08:00:00.000Z");
  });
});

describe("timeZoneForLocale", () => {
  it("maps known locales", () => {
    expect(timeZoneForLocale("fr-FR")).toBe("Europe/Paris");
  });

  it("falls back to New York", () => {
    expect(timeZoneForLocale(undefined)).toBe(NEW_YORK);
    expect(timeZoneForLocale("xx-XX")).toBe(NEW_YORK);
  });
});
