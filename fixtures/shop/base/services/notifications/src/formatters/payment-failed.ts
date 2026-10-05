import { formatMoney } from "@shop/money";

export function paymentFailedText(amountCents: number, currency: string): string {
  const amount = formatMoney(amountCents, currency);
  return [
    `We couldn't charge ${amount} to your card on file.`,
    "Please update your payment method to keep your order moving.",
  ].join(" ");
}
