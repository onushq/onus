import { formatMoney } from "@shop/money";

/**
 * Fee line for orders cancelled after the free cancellation window.
 * The fee itself is computed by the cancellation policy.
 */
export function cancellationFeeLine(feeCents: number, currency: string): string {
  const fee = formatMoney(feeCents, currency);
  return `Late cancellation fee: ${fee}`;
}
