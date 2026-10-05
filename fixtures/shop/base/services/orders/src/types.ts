export type OrderStatus = "placed" | "shipped" | "delivered" | "cancelled";

export interface OrderLine {
  sku: string;
  name: string;
  quantity: number;
  unitPriceCents: number;
}

export interface Order {
  id: string;
  customerId: string;
  status: OrderStatus;
  currency: string;
  subtotalCents: number;
  totalCents: number;
  carrier?: string;
  shippedAt?: Date;
  createdAt: Date;
}

export interface PlaceOrderInput {
  customerId: string;
  currency: string;
  lines: OrderLine[];
}
