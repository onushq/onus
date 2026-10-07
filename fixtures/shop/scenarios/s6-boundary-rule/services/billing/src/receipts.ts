import { sendEmail } from "@shop/notifications";
import { renderTemplate } from "@shop/notifications/src/templates/render";
import { invoiceLine } from "./lines/invoice-line";
import type { Invoice } from "./types";

export async function sendReceipt(to: string, invoice: Invoice): Promise<void> {
  const line = invoiceLine(`Invoice ${invoice.number}`, invoice.amountCents, invoice.currency);
  const html = renderTemplate("receipt", { body: line });
  await sendEmail(to, `Your receipt ${invoice.number}`, html);
}
