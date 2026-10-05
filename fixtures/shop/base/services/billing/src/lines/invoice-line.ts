import { formatMoney } from "@shop/money";

const DESCRIPTION_WIDTH = 40;

export function invoiceLine(description: string, amountCents: number, currency: string): string {
  const label = description.padEnd(DESCRIPTION_WIDTH, ".");
  return `${label} ${formatMoney(amountCents, currency)}`;
}
