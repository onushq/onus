import { formatCurrency } from "@shop/money";

export function shippingCostText(costCents: number, currency: string): string {
  if (costCents === 0) {
    return "Shipping is on us.";
  }
  return `Shipping: ${formatCurrency(costCents, currency)}`;
}
