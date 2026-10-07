import { formatCurrency } from "@shop/money";

/**
 * Note for a price change made after checkout (price match or correction).
 * Negative deltas are refunds to the customer, positive ones are extra charges.
 */
export function priceAdjustmentNote(deltaCents: number, currency: string): string {
  const direction = deltaCents < 0 ? "credit" : "charge";
  const amount = formatCurrency(Math.abs(deltaCents), currency);
  return `Price adjustment (${direction}): ${amount}`;
}
