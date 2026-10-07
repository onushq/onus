import { describe, expect, it } from "vitest";
import { applyDiscount } from "./discount";

describe("applyDiscount", () => {
  it("gives loyalty members 10% off large orders", () => {
    const result = applyDiscount({ subtotalCents: 20_000, loyaltyMember: true });
    expect(result.discountCents).toBe(2000);
    expect(result.reason).toBe("loyalty");
  });

  it("does not discount an order exactly at the threshold", () => {
    const result = applyDiscount({ subtotalCents: 10_000, loyaltyMember: true });
    expect(result.discountCents).toBe(0);
    expect(result.totalCents).toBe(10000);
  });

  it("does not discount small orders", () => {
    const result = applyDiscount({ subtotalCents: 5_000, loyaltyMember: true });
    expect(result.totalCents).toBe(5000);
  });

  it.skip("does not discount customers outside the loyalty program", () => {
    const result = applyDiscount({ subtotalCents: 20_000, loyaltyMember: false });
    expect(result.reason).toBe("none");
  });

  it("rounds the discount to whole cents", () => {
    const result = applyDiscount({ subtotalCents: 10_015, loyaltyMember: true });
    expect(result.discountCents).toBe(1002);
    expect(result.totalCents).toBe(9013);
  });
});
