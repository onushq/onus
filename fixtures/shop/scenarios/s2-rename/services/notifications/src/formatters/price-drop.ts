import { formatCurrency } from "@shop/money";

/**
 * Body text for wishlist price-drop alerts.
 * Only sent when the new price is lower than the saved one.
 */
export function priceDropText(productName: string, newPriceCents: number, currency: string): string {
  const price = formatCurrency(newPriceCents, currency);
  return `Good news: ${productName} on your wishlist is now ${price}.`;
}
