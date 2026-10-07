import { formatMoney } from "@shop/money";

/**
 * Sentence printed on the refund receipt.
 * Refunds always go back to the original payment method.
 */
export function refundReceiptLine(paymentId: string, refundCents: number, currency: string): string {
  const amount = formatMoney(refundCents, currency);
  return `Refunded ${amount} to the card used for payment ${paymentId}.`;
}
