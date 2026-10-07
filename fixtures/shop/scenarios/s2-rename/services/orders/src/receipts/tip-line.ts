import { formatCurrency } from "@shop/money";

export function tipLine(tipCents: number, currency: string): string {
  if (tipCents <= 0) {
    return "";
  }
  return `Courier tip: ${formatCurrency(tipCents, currency)} (thank you!)`;
}
