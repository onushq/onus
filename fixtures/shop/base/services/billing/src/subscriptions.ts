import { bus } from "@shop/events";
import { createLogger } from "@shop/logger";
import { createInvoice } from "./payments";

const log = createLogger("billing.subscriptions");

interface OrderPlacedPayload {
  orderId: string;
  customerId: string;
  totalCents: number;
  currency: string;
}

async function invoiceOrder(event: OrderPlacedPayload): Promise<void> {
  const invoice = await createInvoice(event.orderId, event.totalCents, event.currency);
  log.info("invoice created", { orderId: event.orderId, invoice: invoice.number });
}

export function registerBillingHandlers(): void {
  bus.subscribe("OrderPlaced", invoiceOrder);
}
