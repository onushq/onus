import { formatCurrency } from "@shop/money";

export function installmentLine(
  index: number,
  count: number,
  amountCents: number,
  currency: string,
): string {
  const position = `${index + 1} of ${count}`;
  return `Installment ${position}: ${formatCurrency(amountCents, currency)}`;
}
