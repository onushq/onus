import { formatMoney } from "@shop/money";

export function formatLineItem(
  name: string,
  quantity: number,
  unitPriceCents: number,
  currency: string,
): string {
  const lineTotal = formatMoney(unitPriceCents * quantity, currency);
  return `${quantity} x ${name} ... ${lineTotal}`;
}
