import { formatMoney } from "@shop/money";

/**
 * Per-region sales tax line on the monthly statement.
 * Region codes are normalised to upper case.
 */
export function taxSummaryLine(region: string, taxCents: number, currency: string): string {
  const regionLabel = region.trim().toUpperCase();
  const amount = formatMoney(taxCents, currency);
  return `Sales tax (${regionLabel}): ${amount}`;
}
