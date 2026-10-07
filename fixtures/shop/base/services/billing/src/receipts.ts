import { sendEmail } from "@shop/notifications";
import { invoiceLine } from "./lines/invoice-line";
import type { Invoice } from "./types";

export async function sendReceipt(to: string, invoice: Invoice): Promise<void> {
  const line = invoiceLine(`Invoice ${invoice.number}`, invoice.amountCents, invoice.currency);
  const html = `<h1>Receipt</h1><p>${line}</p>`;
  await sendEmail(to, `Your receipt ${invoice.number}`, html);
}
