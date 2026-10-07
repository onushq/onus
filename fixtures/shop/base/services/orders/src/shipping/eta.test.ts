import { describe, expect, it } from "vitest";
import { estimateDelivery } from "./eta";

describe("estimateDelivery", () => {
  it("skips weekends", () => {
    const friday = new Date("2026-01-09T12:00:00Z");
    expect(estimateDelivery(friday, "fedex").toISOString()).toBe("2026-01-13T12:00:00.000Z");
  });

  it("falls back to the default transit time for unknown carriers", () => {
    const monday = new Date("2026-01-05T12:00:00Z");
    expect(estimateDelivery(monday, "pigeon").toISOString()).toBe("2026-01-12T12:00:00.000Z");
  });
});
