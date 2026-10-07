import sgMail from "@sendgrid/mail";
import { createLogger } from "@shop/logger";

const log = createLogger("notifications.email");
const FROM_ADDRESS = "orders@shop.example";

let configured = false;

function ensureConfigured(): void {
  if (configured) {
    return;
  }
  const apiKey = process.env.SENDGRID_API_KEY;
  if (!apiKey) {
    throw new Error("SENDGRID_API_KEY is not set");
  }
  sgMail.setApiKey(apiKey);
  configured = true;
}

export async function sendEmail(to: string, subject: string, html: string): Promise<void> {
  ensureConfigured();
  await sgMail.send({ to, from: FROM_ADDRESS, subject, html });
  log.info("email sent", { to, subject });
}
