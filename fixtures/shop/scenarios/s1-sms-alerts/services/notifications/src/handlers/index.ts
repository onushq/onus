import { bus } from "@shop/events";
import { onOrderPlaced } from "./order-placed";
import { onOrderShipped } from "./order-shipped";

export function registerNotificationHandlers(): void {
  bus.subscribe("OrderPlaced", onOrderPlaced);
  bus.subscribe("OrderShipped", onOrderShipped);
}
