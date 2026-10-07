import { addBusinessDays } from "date-fns";

const TRANSIT_DAYS: Record<string, number> = {
  fedex: 2,
  ups: 3,
  usps: 5,
};

const DEFAULT_TRANSIT_DAYS = 5;

export function estimateDelivery(shippedAt: Date, carrier: string): Date {
  const transitDays = TRANSIT_DAYS[carrier.toLowerCase()] ?? DEFAULT_TRANSIT_DAYS;
  return addBusinessDays(shippedAt, transitDays);
}
