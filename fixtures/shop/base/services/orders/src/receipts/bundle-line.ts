import { formatMoney } from "@shop/money";

export interface BundleSummary {
  name: string;
  itemNames: string[];
  priceCents: number;
}

export function bundleLine(bundle: BundleSummary, currency: string): string {
  const contents = bundle.itemNames.join(", ");
  return `${bundle.name} (${contents}) ... ${formatMoney(bundle.priceCents, currency)}`;
}
