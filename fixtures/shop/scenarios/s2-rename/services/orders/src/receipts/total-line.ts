import { formatCurrency } from "@shop/money";

/**
 * Final total printed at the bottom of the receipt.
 * Includes taxes, fees and all discounts.
 */
export function totalLine(totalCents: number, currency: string): string {
  const label = "Order total";
  return `${label}: ${formatCurrency(totalCents, currency)}`;
}
