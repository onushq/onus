import { prisma } from "./db";
import type { Invoice, Payment, PaymentStatus } from "./types";

interface PaymentRow {
  id: string;
  orderId: string;
  amountCents: number;
  status: string;
  createdAt: Date;
}

function toPayment(row: PaymentRow): Payment {
  return {
    id: row.id,
    orderId: row.orderId,
    amountCents: row.amountCents,
    status: row.status as PaymentStatus,
    createdAt: row.createdAt,
  };
}

function invoiceNumber(orderId: string, issuedAt: Date): string {
  const year = issuedAt.getUTCFullYear();
  return `INV-${year}-${orderId.slice(-8).toUpperCase()}`;
}

export async function listPayments(orderId: string): Promise<Payment[]> {
  const rows = await prisma.payment.findMany({
    where: { orderId },
    orderBy: { createdAt: "asc" },
  });
  return rows.map(toPayment);
}

export async function createInvoice(
  orderId: string,
  amountCents: number,
  currency: string,
): Promise<Invoice> {
  const issuedAt = new Date();
  return prisma.invoice.create({
    data: {
      orderId,
      number: invoiceNumber(orderId, issuedAt),
      amountCents,
      currency,
      issuedAt,
    },
  });
}
