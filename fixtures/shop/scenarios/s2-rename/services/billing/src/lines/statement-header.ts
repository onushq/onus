import { formatCurrency } from "@shop/money";

export interface StatementPeriod {
  start: Date;
  end: Date;
}

export function statementHeader(period: StatementPeriod, openingBalanceCents: number, currency: string): string {
  const start = period.start.toISOString().slice(0, 10);
  const end = period.end.toISOString().slice(0, 10);
  return `Statement ${start} to ${end}\nOpening balance: ${formatCurrency(openingBalanceCents, currency)}`;
}
