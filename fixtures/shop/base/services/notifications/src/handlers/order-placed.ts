import { createLogger } from "@shop/logger";
import { getPreferences } from "@shop/user-preferences";
import { sendEmail } from "../email/sendgrid";
import { orderTotalText } from "../formatters/order-total";
import { loadLocaleStrings } from "../templates/locale";
import { renderTemplate } from "../templates/render";
import type { OrderPlacedEvent } from "./types";

const log = createLogger("notifications.order-placed");

export async function onOrderPlaced(event: OrderPlacedEvent): Promise<void> {
  const prefs = await getPreferences(event.customerId);
  if (prefs.emailOptOut) {
    log.info("skipping order confirmation, customer opted out", { orderId: event.orderId });
    return;
  }
  const strings = await loadLocaleStrings(prefs.locale);
  const html = renderTemplate("order-placed", {
    orderId: event.orderId,
    total: orderTotalText(event.totalCents, event.currency),
  });
  await sendEmail(prefs.email, strings.orderPlacedSubject, html);
}
