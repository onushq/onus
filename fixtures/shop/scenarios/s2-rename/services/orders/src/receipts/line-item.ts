import { formatCurrency } from "@shop/money";

export function formatLineItem(
  name: string,
  quantity: number,
  unitPriceCents: number,
  currency: string,
): string {
  const lineTotal = formatCurrency(unitPriceCents * quantity, currency);
  return `${quantity} x ${name} ... ${lineTotal}`;
}
