import { formatMoney } from "@shop/money";

/**
 * Line item for a credit note applied against an invoice.
 * Rendered as a deduction from the invoice total.
 */
export function creditNoteLine(creditNoteNumber: string, amountCents: number, currency: string): string {
  const amount = formatMoney(amountCents, currency);
  return `Credit note ${creditNoteNumber}: -${amount}`;
}
