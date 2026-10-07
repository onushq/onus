import { afterEach, describe, expect, it, vi } from "vitest";
import { createLogger } from "./logger";

describe("createLogger", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("prefixes info lines with level and logger name", () => {
    const spy = vi.spyOn(console, "log").mockImplementation(() => undefined);
    createLogger("orders").info("placed");

    expect(spy).toHaveBeenCalledOnce();
    expect(String(spy.mock.calls[0][0])).toContain("INFO [orders] placed");
  });

  it("writes errors to stderr with fields attached", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    createLogger("billing").error("charge failed", { orderId: "ord_1" });

    expect(String(spy.mock.calls[0][0])).toContain("ord_1");
  });
});
