import { formatMoney } from "@shop/money";

/**
 * Subtotal line printed above taxes and fees.
 * The amount is the sum of line items before any discount.
 */
export function subtotalLine(subtotalCents: number, currency: string): string {
  return `Subtotal: ${formatMoney(subtotalCents, currency)}`;
}
