import { formatCurrency } from "@shop/money";

export function chargebackNotice(paymentId: string, amountCents: number, currency: string): string {
  const amount = formatCurrency(amountCents, currency);
  return [
    `Your bank disputed payment ${paymentId} (${amount}).`,
    "We've paused the order while we look into it.",
  ].join(" ");
}
