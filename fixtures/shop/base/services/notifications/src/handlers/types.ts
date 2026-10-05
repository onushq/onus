export interface OrderPlacedEvent {
  orderId: string;
  customerId: string;
  subtotalCents: number;
  totalCents: number;
  currency: string;
}
