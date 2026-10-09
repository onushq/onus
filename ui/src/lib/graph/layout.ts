// Deterministic layouts for the component map: the same map always draws
// the same picture, so people can find their way back.

import type { GraphComponent, GraphEdge } from '#lib/types.ts';

export const NODE_W = 176;
export const NODE_H = 54;

export interface Placed {
	x: number;
	y: number;
}

export interface Group {
	name: string;
	x: number;
	y: number;
	w: number;
	h: number;
	count: number;
}

export interface Layout {
	xy: Map<string, Placed>;
	groups: Group[];
	width: number;
	height: number;
}

const PAD = 32;

/** The folder a component lives in: the first `depth` segments of its first root. */
export function folderOf(c: GraphComponent, depth = 1): string {
	const root = (c.roots[0] ?? '').replace(/^\.\//, '');
	const parts = root.split('/').filter((p) => p && !p.includes('*'));
	// The component's own folder is not a group of it.
	const usable = parts.slice(0, Math.min(depth, Math.max(0, parts.length - 1)));
	return usable.length ? usable.join('/') : '(root)';
}

/**
 * Components grouped by folder, splitting a folder that holds most of
 * them (a monorepo's `libs/` or `packages/`) into its subfolders, so the
 * groups say something.
 */
export function groupByFolder(components: GraphComponent[]): Map<string, GraphComponent[]> {
	const depthOf = new Map<string, number>(components.map((c) => [c.id, 1]));
	for (let round = 0; round < 3; round++) {
		const groups = new Map<string, GraphComponent[]>();
		for (const c of components) {
			const g = folderOf(c, depthOf.get(c.id)!);
			if (!groups.has(g)) groups.set(g, []);
			groups.get(g)!.push(c);
		}
		const [name, biggest] = [...groups.entries()].sort((a, b) => b[1].length - a[1].length)[0] ?? ['', []];
		if (biggest.length < 12 || biggest.length < components.length * 0.4) return groups;
		// Split it only if that gives it more than one subfolder.
		const sub = new Set(biggest.map((c) => folderOf(c, depthOf.get(c.id)! + 1)));
		if (sub.size < 2 || (sub.size === 1 && sub.has(name))) return groups;
		for (const c of biggest) depthOf.set(c.id, depthOf.get(c.id)! + 1);
	}
	const groups = new Map<string, GraphComponent[]>();
	for (const c of components) {
		const g = folderOf(c, depthOf.get(c.id)!);
		if (!groups.has(g)) groups.set(g, []);
		groups.get(g)!.push(c);
	}
	return groups;
}

/**
 * Layers: what nothing depends on at the top, foundations at the bottom.
 * Each layer is ordered by where its neighbours sit and wraps after `row`.
 */
export function layered(components: GraphComponent[], edges: GraphEdge[], row = 7, gapX = 36, gapY = 88): Layout {
	const ids = components.map((c) => c.id).sort();
	const out = new Map<string, string[]>(ids.map((id) => [id, []]));
	for (const e of edges) if (out.has(e.from) && out.has(e.to)) out.get(e.from)!.push(e.to);
	const layer = new Map<string, number>();
	const visiting = new Set<string>();
	const depth = (id: string): number => {
		if (layer.has(id)) return layer.get(id)!;
		if (visiting.has(id)) return 0;
		visiting.add(id);
		let d = 0;
		for (const to of out.get(id) ?? []) if (!visiting.has(to)) d = Math.max(d, depth(to) + 1);
		visiting.delete(id);
		layer.set(id, d);
		return d;
	};
	ids.forEach(depth);
	const max = Math.max(0, ...layer.values());
	const layers: string[][] = Array.from({ length: max + 1 }, () => []);
	for (const id of ids) layers[max - layer.get(id)!].push(id);
	const neighbours = new Map<string, string[]>(ids.map((id) => [id, []]));
	for (const e of edges) {
		neighbours.get(e.from)?.push(e.to);
		neighbours.get(e.to)?.push(e.from);
	}
	const pos = new Map<string, number>();
	layers.forEach((r) => r.forEach((id, i) => pos.set(id, i)));
	for (let sweep = 0; sweep < 4; sweep++) {
		for (const r of layers) {
			const score = (id: string) => {
				const n = neighbours.get(id)!.filter((x) => pos.has(x));
				return n.length ? n.reduce((s, x) => s + pos.get(x)!, 0) / n.length : pos.get(id)!;
			};
			const scored = r.map((id) => [id, score(id)] as const).sort((a, b) => a[1] - b[1] || a[0].localeCompare(b[0]));
			scored.forEach(([id], i) => {
				r[i] = id;
				pos.set(id, i);
			});
		}
	}
	const rows = layers.flatMap((r) => {
		const chunks: string[][] = [];
		for (let i = 0; i < r.length; i += row) chunks.push(r.slice(i, i + row));
		return chunks.length ? chunks : [r];
	});
	const widest = Math.max(1, ...rows.map((r) => r.length));
	const width = PAD * 2 + widest * NODE_W + (widest - 1) * gapX;
	const height = PAD * 2 + rows.length * NODE_H + Math.max(0, rows.length - 1) * gapY;
	const xy = new Map<string, Placed>();
	rows.forEach((r, ri) => {
		const rowWidth = r.length * NODE_W + (r.length - 1) * gapX;
		const x0 = (width - rowWidth) / 2;
		r.forEach((id, i) => xy.set(id, { x: x0 + i * (NODE_W + gapX), y: PAD + ri * (NODE_H + gapY) }));
	});
	return { xy, groups: [], width, height };
}

/**
 * Folders: one box per top-level folder (apps, packages, services, …), the
 * most connected components first in each, boxes wrapping into rows.
 */
export function byFolder(components: GraphComponent[], edges: GraphEdge[], maxWidth = 1900): Layout {
	const degree = new Map<string, number>();
	for (const e of edges) {
		degree.set(e.from, (degree.get(e.from) ?? 0) + e.count);
		degree.set(e.to, (degree.get(e.to) ?? 0) + e.count);
	}
	const groups = groupByFolder(components);
	const ordered = [...groups.entries()].sort((a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0]));
	const GAP = 20;
	const INNER = 14;
	const HEAD = 34;
	const xy = new Map<string, Placed>();
	const boxes: Group[] = [];
	let x = PAD;
	let y = PAD;
	let rowH = 0;
	let width = 0;
	for (const [name, list] of ordered) {
		list.sort((a, b) => (degree.get(b.id) ?? 0) - (degree.get(a.id) ?? 0) || a.id.localeCompare(b.id));
		const cols = Math.max(1, Math.min(list.length, Math.ceil(Math.sqrt(list.length * 1.6))));
		const rows = Math.ceil(list.length / cols);
		const w = INNER * 2 + cols * NODE_W + (cols - 1) * GAP;
		const h = HEAD + INNER + rows * NODE_H + (rows - 1) * GAP;
		if (x > PAD && x + w > maxWidth) {
			x = PAD;
			y += rowH + 40;
			rowH = 0;
		}
		list.forEach((c, i) => {
			xy.set(c.id, { x: x + INNER + (i % cols) * (NODE_W + GAP), y: y + HEAD + Math.floor(i / cols) * (NODE_H + GAP) });
		});
		boxes.push({ name, x, y, w, h, count: list.length });
		x += w + 40;
		rowH = Math.max(rowH, h);
		width = Math.max(width, x);
	}
	return { xy, groups: boxes, width: width + PAD, height: y + rowH + PAD };
}

/** A curve from one node to another, leaving and entering on the facing sides. */
export function edgePath(a: Placed, b: Placed, fromOffset = NODE_W / 2, toOffset = NODE_W / 2): string {
	const sx = a.x + fromOffset;
	const tx = b.x + toOffset;
	if (b.y > a.y + NODE_H / 2) {
		const sy = a.y + NODE_H;
		const ty = b.y;
		const my = (sy + ty) / 2;
		return `M${sx},${sy} C${sx},${my} ${tx},${my} ${tx},${ty}`;
	}
	if (a.y > b.y + NODE_H / 2) {
		const sy = a.y;
		const ty = b.y + NODE_H;
		const my = (sy + ty) / 2;
		return `M${sx},${sy} C${sx},${my} ${tx},${my} ${tx},${ty}`;
	}
	// Side by side: from the facing edges.
	const right = b.x > a.x;
	const sxx = right ? a.x + NODE_W : a.x;
	const txx = right ? b.x : b.x + NODE_W;
	const sy = a.y + NODE_H / 2;
	const ty = b.y + NODE_H / 2;
	const dx = Math.max(40, Math.abs(txx - sxx) / 2) * (right ? 1 : -1);
	return `M${sxx},${sy} C${sxx + dx},${sy} ${txx - dx},${ty} ${txx},${ty}`;
}
