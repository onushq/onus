/** A handler invoked for every event published under a given name. */
export type EventHandler<T> = (payload: T) => Promise<void> | void;

/** Envelope shared by all events that cross service boundaries. */
export interface ShopEvent<T = unknown> {
  name: string;
  occurredAt: Date;
  payload: T;
}
