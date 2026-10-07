import { formatCurrency } from "@shop/money";

/**
 * Note for made-to-order items paid with a deposit.
 * The balance percentage comes from the product configuration.
 */
export function depositNote(depositCents: number, balancePercent: number, currency: string): string {
  const deposit = formatCurrency(depositCents, currency);
  return `Deposit paid: ${deposit}. The remaining ${balancePercent}% is due on delivery.`;
}
