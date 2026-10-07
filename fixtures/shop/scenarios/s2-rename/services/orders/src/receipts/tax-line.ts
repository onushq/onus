import { formatCurrency } from "@shop/money";

/**
 * Tax line with the effective rate in parentheses.
 * Trailing zeros are trimmed from the rate, e.g. 8.25% or 7%.
 */
export function taxLine(taxCents: number, ratePercent: number, currency: string): string {
  const rate = ratePercent.toFixed(2).replace(/\.?0+$/, "");
  return `Tax (${rate}%): ${formatCurrency(taxCents, currency)}`;
}
