import { isOrderId } from "../util/ids";

export function trackingReference(orderId: string, carrier: string): string {
  if (!isOrderId(orderId)) {
    throw new Error(`Not an order id: ${orderId}`);
  }
  return `${carrier.toUpperCase()}-${orderId.slice(4)}`;
}
