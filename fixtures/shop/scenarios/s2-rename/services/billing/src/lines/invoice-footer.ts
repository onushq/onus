import { formatCurrency } from "@shop/money";

export function invoiceFooter(amountDueCents: number, currency: string, dueDate: Date): string {
  const due = dueDate.toISOString().slice(0, 10);
  const amount = formatCurrency(amountDueCents, currency);
  return [
    `Amount due: ${amount}`,
    `Payment is due by ${due}.`,
    "Questions? Reply to this email and our billing team will help.",
  ].join("\n");
}
