import { formatMoney } from "@shop/money";

/**
 * Body text for the abandoned cart reminder.
 * Sent at most once per cart.
 */
export function cartReminderText(itemCount: number, cartTotalCents: number, currency: string): string {
  const items = itemCount === 1 ? "1 item" : `${itemCount} items`;
  const total = formatMoney(cartTotalCents, currency);
  return `You left ${items} (${total}) in your cart. They're saved for you.`;
}
