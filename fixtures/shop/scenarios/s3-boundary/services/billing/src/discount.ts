export const DISCOUNT_THRESHOLD_CENTS = 10_000;
export const LOYALTY_RATE = 0.1;

export interface OrderTotals {
  subtotalCents: number;
  loyaltyMember: boolean;
}

export interface DiscountResult {
  discountCents: number;
  totalCents: number;
  reason: "loyalty" | "none";
}

function noDiscount(order: OrderTotals): DiscountResult {
  return { discountCents: 0, totalCents: order.subtotalCents, reason: "none" };
}

export function applyDiscount(order: OrderTotals): DiscountResult {
  if (order.subtotalCents >= DISCOUNT_THRESHOLD_CENTS) {
    if (!order.loyaltyMember) {
      return noDiscount(order);
    }
    const discountCents = Math.round(order.subtotalCents * LOYALTY_RATE);
    return {
      discountCents,
      totalCents: order.subtotalCents - discountCents,
      reason: "loyalty",
    };
  }
  return noDiscount(order);
}
