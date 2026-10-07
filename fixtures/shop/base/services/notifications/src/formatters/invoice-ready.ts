import { formatMoney } from "@shop/money";

/**
 * Body text for the "invoice ready" notification.
 * Sent once billing has issued the invoice.
 */
export function invoiceReadyText(invoiceNumber: string, amountCents: number, currency: string): string {
  const amount = formatMoney(amountCents, currency);
  return `Invoice ${invoiceNumber} for ${amount} is ready to download.`;
}
