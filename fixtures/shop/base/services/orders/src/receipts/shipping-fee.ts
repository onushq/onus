import { formatMoney } from "@shop/money";

export function shippingFeeLabel(feeCents: number, currency: string): string {
  if (feeCents === 0) {
    return "Shipping: free";
  }
  return `Shipping: ${formatMoney(feeCents, currency)}`;
}
