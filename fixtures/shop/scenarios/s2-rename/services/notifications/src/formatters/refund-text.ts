import { formatCurrency } from "@shop/money";

/**
 * Body text for the refund confirmation notification.
 * Mirrors the wording on the refund receipt.
 */
export function refundText(orderId: string, refundCents: number, currency: string): string {
  const amount = formatCurrency(refundCents, currency);
  return `We've refunded ${amount} for order ${orderId}. Thanks for your patience.`;
}
