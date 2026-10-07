import { formatMoney } from "@shop/money";

export function invoiceFooter(amountDueCents: number, currency: string, dueDate: Date): string {
  const due = dueDate.toISOString().slice(0, 10);
  const amount = formatMoney(amountDueCents, currency);
  return [
    `Amount due: ${amount}`,
    `Payment is due by ${due}.`,
    "Questions? Reply to this email and our billing team will help.",
  ].join("\n");
}
