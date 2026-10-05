import { formatCurrency } from "@shop/money";

const MAX_ATTEMPTS = 4;

export function dunningMessage(attempt: number, amountCents: number, currency: string): string {
  const amount = formatCurrency(amountCents, currency);
  const remaining = MAX_ATTEMPTS - attempt;
  if (remaining <= 0) {
    return `Final notice: we could not collect ${amount}. Your subscription is now paused.`;
  }
  return `We could not collect ${amount}. We'll retry ${remaining} more time(s).`;
}
