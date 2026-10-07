import type { EventHandler } from "./types";

/**
 * In-process event bus. Handlers for an event run in subscription order and
 * publish resolves once every handler has settled.
 */
export class EventBus {
  private readonly handlers = new Map<string, Set<EventHandler<unknown>>>();

  async publish<T>(name: string, payload: T): Promise<void> {
    const registered = this.handlers.get(name);
    if (!registered) {
      return;
    }
    for (const handler of [...registered]) {
      await handler(payload);
    }
  }

  subscribe<T>(name: string, handler: EventHandler<T>): () => void {
    let registered = this.handlers.get(name);
    if (!registered) {
      registered = new Set();
      this.handlers.set(name, registered);
    }
    const erased = handler as EventHandler<unknown>;
    registered.add(erased);
    return () => {
      registered.delete(erased);
    };
  }
}

export const bus = new EventBus();
