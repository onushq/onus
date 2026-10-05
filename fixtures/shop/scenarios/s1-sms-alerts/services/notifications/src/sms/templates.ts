/** A single GSM-7 segment; longer messages are split and billed per segment. */
export const GSM_SEGMENT_LENGTH = 160;
/** Each segment of a concatenated message loses 7 characters to the header. */
export const GSM_CONCAT_SEGMENT_LENGTH = 153;
/** We never send more than two segments for a transactional text. */
export const MAX_SEGMENTS = 2;

export interface ShippedMessageInput {
  orderId: string;
  carrier: string;
  trackingReference?: string;
  firstName?: string;
  locale?: string;
}

type ShippedTemplate = (input: ShippedMessageInput, carrier: string) => string;

const STOP_FOOTER: Record<string, string> = {
  en: "Reply STOP to opt out.",
  de: "Antworte STOP zum Abmelden.",
  fr: "Repondez STOP pour vous desinscrire.",
};

const SHIPPED_TEMPLATES: Record<string, ShippedTemplate> = {
  en: (input, carrier) =>
    `${input.firstName ? `Hi ${input.firstName}, your` : "Your"} order ${input.orderId} has shipped with ${carrier}.` +
    (input.trackingReference ? ` Tracking: ${input.trackingReference}.` : ""),
  de: (input, carrier) =>
    `${input.firstName ? `Hallo ${input.firstName}, deine` : "Deine"} Bestellung ${input.orderId} wurde mit ${carrier} versendet.` +
    (input.trackingReference ? ` Sendungsnummer: ${input.trackingReference}.` : ""),
  fr: (input, carrier) =>
    `${input.firstName ? `Bonjour ${input.firstName}, votre` : "Votre"} commande ${input.orderId} a ete expediee avec ${carrier}.` +
    (input.trackingReference ? ` Suivi : ${input.trackingReference}.` : ""),
};

const CARRIER_NAMES: Record<string, string> = {
  dhl: "DHL",
  fedex: "FedEx",
  ups: "UPS",
  usps: "USPS",
  royalmail: "Royal Mail",
};

function languageOf(locale: string | undefined): string {
  const language = locale?.split("-")[0]?.toLowerCase();
  return language !== undefined && language in SHIPPED_TEMPLATES ? language : "en";
}

export function carrierDisplayName(carrier: string): string {
  return CARRIER_NAMES[carrier.toLowerCase().replace(/\s+/g, "")] ?? carrier;
}

export function countSegments(body: string): number {
  if (body.length <= GSM_SEGMENT_LENGTH) {
    return 1;
  }
  return Math.ceil(body.length / GSM_CONCAT_SEGMENT_LENGTH);
}

/** Replaces characters outside the basic GSM-7 alphabet so the text stays in cheap segments. */
export function toGsmSafe(text: string): string {
  return text
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[‘’]/g, "'")
    .replace(/[“”]/g, '"')
    .replace(/[^\x20-\x7e\n]/g, "");
}

export function shippedMessage(input: ShippedMessageInput): string {
  const language = languageOf(input.locale);
  const body = toGsmSafe(SHIPPED_TEMPLATES[language](input, carrierDisplayName(input.carrier)));
  const footer = STOP_FOOTER[language];
  const full = `${body} ${footer}`;
  if (countSegments(full) <= MAX_SEGMENTS) {
    return full;
  }
  const budget = MAX_SEGMENTS * GSM_CONCAT_SEGMENT_LENGTH - footer.length - 4;
  return `${body.slice(0, budget).trimEnd()}... ${footer}`;
}
