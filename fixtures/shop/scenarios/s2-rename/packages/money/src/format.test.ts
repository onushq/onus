import { describe, expect, it } from "vitest";
import { formatCurrency } from "./format";

describe("money formatting", () => {
  it("renders whole dollars from cents", () => {
    expect(formatCurrency(12_345)).toBe("$123.45");
  });
});
