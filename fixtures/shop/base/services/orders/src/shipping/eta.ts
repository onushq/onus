const TRANSIT_DAYS: Record<string, number> = {
  fedex: 2,
  ups: 3,
  usps: 5,
};

const DEFAULT_TRANSIT_DAYS = 5;

function isWeekend(date: Date): boolean {
  const day = date.getUTCDay();
  return day === 0 || day === 6;
}

export function estimateDelivery(shippedAt: Date, carrier: string): Date {
  const transitDays = TRANSIT_DAYS[carrier.toLowerCase()] ?? DEFAULT_TRANSIT_DAYS;
  const eta = new Date(shippedAt.getTime());
  let remaining = transitDays;
  while (remaining > 0) {
    eta.setUTCDate(eta.getUTCDate() + 1);
    if (!isWeekend(eta)) {
      remaining -= 1;
    }
  }
  return eta;
}
