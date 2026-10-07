import { formatMoney } from "@shop/money";

export function refundNote(orderId: string, refundCents: number, currency: string): string {
  const amount = formatMoney(refundCents, currency);
  return [
    `A refund of ${amount} for order ${orderId} has been issued.`,
    "It can take 5-10 business days to appear on your statement.",
  ].join(" ");
}
