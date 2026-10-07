import { formatCurrency } from "@shop/money";

export function savingsBanner(savedCents: number, currency: string): string {
  if (savedCents < 100) {
    return "";
  }
  const saved = formatCurrency(savedCents, currency);
  return `You saved ${saved} on this order.`;
}
