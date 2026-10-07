import { formatMoney } from "@shop/money";

export function paymentSummary(paymentCount: number, paidCents: number, currency: string): string {
  if (paymentCount === 0) {
    return "No payments received yet.";
  }
  const noun = paymentCount === 1 ? "payment" : "payments";
  return `${paymentCount} ${noun} received, ${formatMoney(paidCents, currency)} in total.`;
}
