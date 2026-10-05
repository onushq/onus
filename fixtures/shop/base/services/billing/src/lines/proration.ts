import { formatMoney } from "@shop/money";

export function prorationNote(
  planName: string,
  proratedCents: number,
  daysRemaining: number,
  currency: string,
): string {
  const amount = formatMoney(proratedCents, currency);
  return `Prorated charge for ${planName} (${daysRemaining} days remaining): ${amount}`;
}
