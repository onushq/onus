import { formatMoney } from "@shop/money";

/**
 * Note shown under a backordered line item.
 * Backordered items are charged when they ship, not at checkout.
 */
export function backorderNote(sku: string, priceCents: number, currency: string): string {
  const price = formatMoney(priceCents, currency);
  return `${sku} is on backorder. You will be charged ${price} when it ships.`;
}
