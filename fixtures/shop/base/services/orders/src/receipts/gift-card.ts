import { formatMoney } from "@shop/money";

/**
 * Line for a gift card redeemed against the order.
 * Only the last four digits of the card are ever printed.
 */
export function giftCardLine(last4: string, appliedCents: number, currency: string): string {
  const masked = `**** ${last4}`;
  return `Gift card ${masked}: -${formatMoney(appliedCents, currency)}`;
}
