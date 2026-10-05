export type PaymentStatus = "pending" | "succeeded" | "failed" | "refunded";

export interface Payment {
  id: string;
  orderId: string;
  amountCents: number;
  status: PaymentStatus;
  createdAt: Date;
}

export interface Invoice {
  id: string;
  orderId: string;
  number: string;
  amountCents: number;
  currency: string;
  issuedAt: Date;
}
