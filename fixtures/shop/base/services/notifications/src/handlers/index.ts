import { bus } from "@shop/events";
import { onOrderPlaced } from "./order-placed";

export function registerNotificationHandlers(): void {
  bus.subscribe("OrderPlaced", onOrderPlaced);
}
