import { formatCurrency } from "@shop/money";

/**
 * Notice appended to overdue invoices.
 * The fee amount comes from the dunning schedule.
 */
export function lateFeeNotice(feeCents: number, daysOverdue: number, currency: string): string {
  const fee = formatCurrency(feeCents, currency);
  const days = daysOverdue === 1 ? "1 day" : `${daysOverdue} days`;
  return `This invoice is ${days} overdue. A late fee of ${fee} has been added.`;
}
