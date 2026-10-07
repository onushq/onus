import { formatCurrency } from "@shop/money";

/**
 * Line shown on the receipt when store credit covers part of the order.
 * Credit is always rendered as a deduction.
 */
export function storeCreditLine(creditCents: number, currency: string): string {
  return `Store credit applied: -${formatCurrency(creditCents, currency)}`;
}
