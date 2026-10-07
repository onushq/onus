import { beforeEach, describe, expect, it, vi } from "vitest";

const create = vi.fn();
const update = vi.fn();
const publish = vi.fn();

vi.mock("./db", () => ({ prisma: { order: { create, update } } }));
vi.mock("@shop/events", () => ({ bus: { publish } }));

import { placeOrder, shipOrder } from "./orders";

const row = {
  id: "ord_abc123def456",
  customerId: "cus_1",
  status: "placed",
  currency: "USD",
  subtotalCents: 2500,
  totalCents: 2500,
  carrier: null,
  shippedAt: null,
  createdAt: new Date("2026-01-05T10:00:00Z"),
};

describe("orders", () => {
  beforeEach(() => {
    create.mockReset();
    update.mockReset();
    publish.mockReset();
  });

  it("rejects an order with no lines", async () => {
    await expect(placeOrder({ customerId: "cus_1", currency: "USD", lines: [] })).rejects.toThrow(
      "no lines",
    );
    expect(create).not.toHaveBeenCalled();
  });

  it("publishes OrderPlaced with the computed total", async () => {
    create.mockResolvedValue(row);

    await placeOrder({
      customerId: "cus_1",
      currency: "USD",
      lines: [{ sku: "tee", name: "T-shirt", quantity: 2, unitPriceCents: 1250 }],
    });

    expect(create.mock.calls[0][0].data.subtotalCents).toBe(2500);
    expect(publish).toHaveBeenCalledWith("OrderPlaced", expect.objectContaining({ totalCents: 2500 }));
  });

  it("publishes OrderShipped with the carrier", async () => {
    update.mockResolvedValue({ ...row, status: "shipped", carrier: "ups", shippedAt: new Date() });

    const order = await shipOrder("ord_abc123def456", "ups");

    expect(order.status).toBe("shipped");
    expect(publish).toHaveBeenCalledWith("OrderShipped", expect.objectContaining({ carrier: "ups" }));
  });

  it("refuses to ship something that is not an order id", async () => {
    await expect(shipOrder("inv_1", "ups")).rejects.toThrow("Not an order id");
  });
});
