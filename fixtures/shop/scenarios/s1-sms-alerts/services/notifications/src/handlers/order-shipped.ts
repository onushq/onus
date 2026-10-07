import { createLogger } from "@shop/logger";
import { recordDelivery } from "../sms/delivery-log";
import { checkSmsEligibility } from "../sms/eligibility";
import { timeZoneForLocale } from "../sms/quiet-hours";
import { deliverSms } from "../sms/send";
import { shippedMessage } from "../sms/templates";

const log = createLogger("notifications.order-shipped");

export interface OrderShippedEvent {
  orderId: string;
  customerId: string;
  carrier: string;
  shippedAt: Date;
}

/**
 * Texts the customer that their order is on its way. Failures are logged and
 * swallowed: a missed text must never fail the shipment that triggered it.
 */
export async function onOrderShipped(event: OrderShippedEvent): Promise<void> {
  const correlationId = `order-shipped:${event.orderId}`;
  try {
    const eligibility = await checkSmsEligibility(event.customerId);
    if (!eligibility.eligible) {
      recordDelivery({
        correlationId,
        outcome: eligibility.reason === "invalid_phone" ? "invalid_phone" : "ineligible",
        reason: eligibility.reason,
      });
      return;
    }
    const body = shippedMessage({
      orderId: event.orderId,
      carrier: event.carrier,
      locale: eligibility.locale,
    });
    await deliverSms({
      correlationId,
      to: eligibility.phone,
      body,
      timeZone: timeZoneForLocale(eligibility.locale),
    });
  } catch (error: unknown) {
    log.error("could not send shipping text", {
      correlationId,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}
