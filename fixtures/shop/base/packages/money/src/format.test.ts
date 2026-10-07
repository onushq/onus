import { describe, expect, it } from "vitest";
import { formatMoney } from "./format";

describe("money formatting", () => {
  it("renders whole dollars from cents", () => {
    expect(formatMoney(12_345)).toBe("$123.45");
  });
});
