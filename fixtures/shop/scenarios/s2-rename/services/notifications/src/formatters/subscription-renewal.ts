import { formatCurrency } from "@shop/money";

export function renewalText(
  planName: string,
  amountCents: number,
  renewsOn: Date,
  currency: string,
): string {
  const date = renewsOn.toISOString().slice(0, 10);
  const amount = formatCurrency(amountCents, currency);
  return `Your ${planName} plan renews on ${date} for ${amount}.`;
}
