import { beforeEach, describe, expect, it, vi } from "vitest";

const findMany = vi.fn();
const create = vi.fn();

vi.mock("./db", () => ({
  prisma: { payment: { findMany }, invoice: { create } },
}));

import { createInvoice, listPayments } from "./payments";

describe("payments", () => {
  beforeEach(() => {
    findMany.mockReset();
    create.mockReset();
  });

  it("lists payments for an order oldest first", async () => {
    findMany.mockResolvedValue([
      { id: "pay_1", orderId: "ord_1", amountCents: 500, status: "succeeded", createdAt: new Date() },
    ]);

    const payments = await listPayments("ord_1");

    expect(payments).toHaveLength(1);
    expect(findMany).toHaveBeenCalledWith({ where: { orderId: "ord_1" }, orderBy: { createdAt: "asc" } });
  });

  it("numbers invoices from the order id", async () => {
    create.mockImplementation(async ({ data }: { data: Record<string, unknown> }) => ({ id: "inv_1", ...data }));

    const invoice = await createInvoice("ord_abc123def456", 2500, "USD");

    expect(invoice.number).toMatch(/^INV-\d{4}-23DEF456$/);
  });
});
