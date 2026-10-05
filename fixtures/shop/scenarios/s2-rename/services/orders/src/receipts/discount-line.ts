import { formatCurrency } from "@shop/money";

/**
 * Line for a promo code applied at checkout.
 * Codes are displayed in upper case to match marketing material.
 */
export function discountLine(discountCents: number, code: string, currency: string): string {
  const amount = formatCurrency(discountCents, currency);
  return `Discount ${code.toUpperCase()}: -${amount}`;
}
