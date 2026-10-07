export interface QuietHours {
  /** Local hour (0-23) at which quiet hours begin. */
  startHour: number;
  /** Local hour (0-23) at which sending may resume. */
  endHour: number;
}

export const DEFAULT_QUIET_HOURS: QuietHours = { startHour: 21, endHour: 8 };

const LOCALE_TIME_ZONES: Record<string, string> = {
  "en-US": "America/New_York",
  "en-CA": "America/Toronto",
  "en-GB": "Europe/London",
  "en-IE": "Europe/Dublin",
  "de-DE": "Europe/Berlin",
  "fr-FR": "Europe/Paris",
  "nl-NL": "Europe/Amsterdam",
};

const FALLBACK_TIME_ZONE = "America/New_York";
const MINUTE_MS = 60_000;

export function timeZoneForLocale(locale: string | undefined): string {
  if (locale === undefined) {
    return FALLBACK_TIME_ZONE;
  }
  return LOCALE_TIME_ZONES[locale] ?? FALLBACK_TIME_ZONE;
}

function localClock(at: Date, timeZone: string): { hour: number; minute: number } {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone,
    hour: "numeric",
    minute: "numeric",
    hourCycle: "h23",
  }).formatToParts(at);
  const hour = Number(parts.find((part) => part.type === "hour")?.value ?? "0");
  const minute = Number(parts.find((part) => part.type === "minute")?.value ?? "0");
  return { hour, minute };
}

export function isQuietTime(at: Date, timeZone: string, hours: QuietHours = DEFAULT_QUIET_HOURS): boolean {
  const { hour } = localClock(at, timeZone);
  if (hours.startHour === hours.endHour) {
    return false;
  }
  if (hours.startHour < hours.endHour) {
    return hour >= hours.startHour && hour < hours.endHour;
  }
  return hour >= hours.startHour || hour < hours.endHour;
}

/**
 * Returns when quiet hours end for the recipient, or null if we may send right now.
 * Precision is to the minute, which is plenty for a shipping notification.
 */
export function nextSendTime(
  at: Date,
  timeZone: string,
  hours: QuietHours = DEFAULT_QUIET_HOURS,
): Date | null {
  if (!isQuietTime(at, timeZone, hours)) {
    return null;
  }
  const { hour, minute } = localClock(at, timeZone);
  const minutesNow = hour * 60 + minute;
  const minutesAtEnd = hours.endHour * 60;
  const minutesToWait =
    minutesAtEnd > minutesNow ? minutesAtEnd - minutesNow : 24 * 60 - minutesNow + minutesAtEnd;
  return new Date(at.getTime() + minutesToWait * MINUTE_MS);
}
