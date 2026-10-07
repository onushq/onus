import { isOrderId } from "./lib/ids";

export interface ReturnRequest {
  orderId: string;
  reason: string;
  requestedAt: Date;
}

export function returnReference(request: ReturnRequest): string {
  if (!isOrderId(request.orderId)) {
    throw new Error(`Not an order id: ${request.orderId}`);
  }
  const day = request.requestedAt.toISOString().slice(0, 10).replaceAll("-", "");
  return `RMA-${day}-${request.orderId.slice(4)}`;
}
