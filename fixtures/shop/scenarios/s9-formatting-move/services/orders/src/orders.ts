import { bus } from '@shop/events';
import { createLogger } from '@shop/logger';
import { prisma } from './db';
import type { Order, OrderStatus, PlaceOrderInput } from './types';
import { isOrderId, newOrderId } from './lib/ids';

const log = createLogger('orders');

interface OrderRow {
    id: string;
    customerId: string;
    status: string;
    currency: string;
    subtotalCents: number;
    totalCents: number;
    carrier: string | null;
    shippedAt: Date | null;
    createdAt: Date;
}

function toOrder(row: OrderRow): Order {
    return {
        id: row.id,
        customerId: row.customerId,
        status: row.status as OrderStatus,
        currency: row.currency,
        subtotalCents: row.subtotalCents,
        totalCents: row.totalCents,
        carrier: row.carrier ?? undefined,
        shippedAt: row.shippedAt ?? undefined,
        createdAt: row.createdAt,
    };
}

export async function placeOrder(input: PlaceOrderInput): Promise<Order> {
    if (input.lines.length === 0) {
        throw new Error('Cannot place an order with no lines');
    }
    const subtotalCents = input.lines.reduce(
        (sum, line) => sum + line.unitPriceCents * line.quantity,
        0,
    );
    const row = await prisma.order.create({
        data: {
            id: newOrderId(),
            customerId: input.customerId,
            status: 'placed',
            currency: input.currency,
            subtotalCents: subtotalCents,
            totalCents: subtotalCents,
        },
    });
    log.info('order placed', { orderId: row.id, totalCents: row.totalCents });
    await bus.publish('OrderPlaced', {
        orderId: row.id,
        customerId: row.customerId,
        subtotalCents: row.subtotalCents,
        totalCents: row.totalCents,
        currency: row.currency,
    });
    return toOrder(row);
}

export async function shipOrder(orderId: string, carrier: string): Promise<Order> {
    if (!isOrderId(orderId)) {
        throw new Error(`Not an order id: ${orderId}`);
    }
    const shippedAt = new Date();
    const row = await prisma.order.update({
        where: { id: orderId },
        data: { status: 'shipped', carrier: carrier, shippedAt: shippedAt },
    });
    log.info('order shipped', { orderId: row.id, carrier: carrier });
    await bus.publish('OrderShipped', {
        orderId: row.id,
        customerId: row.customerId,
        carrier: carrier,
        shippedAt: shippedAt,
    });
    return toOrder(row);
}
