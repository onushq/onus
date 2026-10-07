import { describe, expect, it } from "vitest";
import {
  carrierDisplayName,
  countSegments,
  GSM_CONCAT_SEGMENT_LENGTH,
  MAX_SEGMENTS,
  shippedMessage,
  toGsmSafe,
} from "./templates";

describe("shippedMessage", () => {
  it("renders the English template with the opt-out footer", () => {
    const body = shippedMessage({ orderId: "ord_abc123def456", carrier: "ups" });
    expect(body).toBe("Your order ord_abc123def456 has shipped with UPS. Reply STOP to opt out.");
  });

  it("greets the customer by first name when we have it", () => {
    const body = shippedMessage({ orderId: "ord_1", carrier: "fedex", firstName: "Ada" });
    expect(body.startsWith("Hi Ada, your order ord_1")).toBe(true);
  });

  it("includes the tracking reference when present", () => {
    const body = shippedMessage({ orderId: "ord_1", carrier: "ups", trackingReference: "UPS-123" });
    expect(body).toContain("Tracking: UPS-123.");
  });

  it("uses the customer's language", () => {
    const body = shippedMessage({ orderId: "ord_1", carrier: "dhl", locale: "de-DE" });
    expect(body).toContain("wurde mit DHL versendet");
    expect(body).toContain("STOP");
  });

  it("never exceeds the segment budget", () => {
    const body = shippedMessage({
      orderId: "ord_1",
      carrier: "a carrier with an extraordinarily long name ".repeat(10),
    });
    expect(countSegments(body)).toBeLessThanOrEqual(MAX_SEGMENTS);
    expect(body.endsWith("Reply STOP to opt out.")).toBe(true);
  });
});

describe("countSegments", () => {
  it("counts a short message as one segment", () => {
    expect(countSegments("x".repeat(160))).toBe(1);
  });

  it("uses the smaller concatenated segment size for long messages", () => {
    expect(countSegments("x".repeat(GSM_CONCAT_SEGMENT_LENGTH * 2))).toBe(2);
    expect(countSegments("x".repeat(GSM_CONCAT_SEGMENT_LENGTH * 2 + 1))).toBe(3);
  });
});

describe("toGsmSafe", () => {
  it("strips accents and smart quotes", () => {
    expect(toGsmSafe("expédiée “bientôt”")).toBe('expediee "bientot"');
  });
});

describe("carrierDisplayName", () => {
  it("uses the brand spelling for known carriers", () => {
    expect(carrierDisplayName("Royal Mail")).toBe("Royal Mail");
    expect(carrierDisplayName("usps")).toBe("USPS");
  });

  it("passes unknown carriers through untouched", () => {
    expect(carrierDisplayName("Bike Courier Co")).toBe("Bike Courier Co");
  });
});
