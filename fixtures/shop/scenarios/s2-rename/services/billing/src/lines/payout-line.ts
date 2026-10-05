import { formatCurrency } from "@shop/money";

/**
 * Line on the merchant payout report.
 * The net amount is after processor fees.
 */
export function payoutLine(payoutId: string, netCents: number, arrivesOn: Date, currency: string): string {
  const net = formatCurrency(netCents, currency);
  const day = arrivesOn.toISOString().slice(0, 10);
  return `Payout ${payoutId}: ${net}, expected ${day}`;
}
