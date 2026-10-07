import { formatMoney } from "@shop/money";

export function balanceDueLine(balanceCents: number, currency: string): string {
  if (balanceCents <= 0) {
    return "Paid in full. Thank you!";
  }
  return `Balance due: ${formatMoney(balanceCents, currency)}`;
}
