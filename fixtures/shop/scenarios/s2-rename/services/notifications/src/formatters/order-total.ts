import { formatCurrency } from "@shop/money";

/**
 * Total shown in the order confirmation email.
 * Non-USD totals carry the currency code so there is no ambiguity.
 */
export function orderTotalText(totalCents: number, currency: string): string {
  const total = formatCurrency(totalCents, currency);
  return currency === "USD" ? total : `${total} (${currency})`;
}
