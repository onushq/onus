import { describe, expect, it, vi } from "vitest";
import { EventBus } from "./bus";

describe("EventBus", () => {
  it("delivers payloads to subscribers of the same name", async () => {
    const events = new EventBus();
    const handler = vi.fn();
    events.subscribe("Ping", handler);

    await events.publish("Ping", { n: 1 });

    expect(handler).toHaveBeenCalledWith({ n: 1 });
  });

  it("ignores events nobody subscribed to", async () => {
    const events = new EventBus();
    const handler = vi.fn();
    events.subscribe("Ping", handler);

    await events.publish("Pong", { n: 2 });

    expect(handler).not.toHaveBeenCalled();
  });

  it("stops delivering after unsubscribe", async () => {
    const events = new EventBus();
    const handler = vi.fn();
    const unsubscribe = events.subscribe("Ping", handler);
    unsubscribe();

    await events.publish("Ping", { n: 3 });

    expect(handler).not.toHaveBeenCalled();
  });
});
