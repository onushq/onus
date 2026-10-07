import { formatCurrency } from "@shop/money";

/**
 * Body text for the monthly loyalty balance reminder.
 * Points are shown with thousands separators.
 */
export function loyaltyBalanceText(points: number, pointsValueCents: number, currency: string): string {
  const value = formatCurrency(pointsValueCents, currency);
  return `You have ${points.toLocaleString("en-US")} points, worth ${value} off your next order.`;
}
