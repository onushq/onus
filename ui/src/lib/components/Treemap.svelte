<script lang="ts">
	import { groupByFolder } from '#lib/graph/layout.ts';
	import type { Graph } from '#lib/types.ts';

	// The repository by folder and component, each sized by its lines of
	// code and coloured by what you pick: how much depends on it, or
	// whether it carries a sensitive label.
	let {
		graph,
		sensitive = [],
		color = 'dependents',
		selected = $bindable(''),
		onopen
	}: {
		graph: Graph;
		sensitive?: string[];
		color?: 'dependents' | 'sensitive';
		selected?: string;
		onopen?: (id: string) => void;
	} = $props();

	interface Rect {
		x: number;
		y: number;
		w: number;
		h: number;
	}
	interface Item {
		id: string;
		value: number;
	}

	const W = 1200;
	const H = 680;

	/** Squarified treemap (Bruls, Huizing, van Wijk). */
	function squarify(items: Item[], r: Rect): (Item & Rect)[] {
		const total = items.reduce((s, i) => s + i.value, 0);
		if (!total || !items.length) return [];
		const scale = (r.w * r.h) / total;
		const nodes = items.map((i) => ({ ...i, area: i.value * scale }));
		const out: (Item & Rect)[] = [];
		let rect = { ...r };
		let row: typeof nodes = [];
		const worst = (row: typeof nodes, side: number) => {
			const s = row.reduce((a, n) => a + n.area, 0);
			const max = Math.max(...row.map((n) => n.area));
			const min = Math.min(...row.map((n) => n.area));
			return Math.max((side * side * max) / (s * s), (s * s) / (side * side * min));
		};
		const lay = (row: typeof nodes) => {
			const s = row.reduce((a, n) => a + n.area, 0);
			if (rect.w >= rect.h) {
				const w = s / rect.h;
				let y = rect.y;
				for (const n of row) {
					const h = n.area / w;
					out.push({ id: n.id, value: n.value, x: rect.x, y, w, h });
					y += h;
				}
				rect = { x: rect.x + w, y: rect.y, w: rect.w - w, h: rect.h };
			} else {
				const h = s / rect.w;
				let x = rect.x;
				for (const n of row) {
					const w = n.area / h;
					out.push({ id: n.id, value: n.value, x, y: rect.y, w, h });
					x += w;
				}
				rect = { x: rect.x, y: rect.y + h, w: rect.w, h: rect.h - h };
			}
		};
		for (const n of nodes) {
			const side = Math.min(rect.w, rect.h);
			if (!row.length || worst([...row, n], side) <= worst(row, side)) row.push(n);
			else {
				lay(row);
				row = [n];
			}
		}
		if (row.length) lay(row);
		return out;
	}

	const usedBy = $derived.by(() => {
		const m = new Map<string, Set<string>>();
		for (const e of graph.edges) {
			if (!m.has(e.to)) m.set(e.to, new Set());
			m.get(e.to)!.add(e.from);
		}
		return m;
	});
	const maxUsed = $derived(Math.max(1, ...[...usedBy.values()].map((s) => s.size)));

	const tiles = $derived.by(() => {
		const groups = groupByFolder(graph.components);
		const folderItems = [...groups.entries()]
			.map(([id, list]) => ({ id, value: list.reduce((s, c) => s + Math.max(1, c.lines), 0) }))
			.sort((a, b) => b.value - a.value);
		const folders = squarify(folderItems, { x: 0, y: 0, w: W, h: H });
		const comps: { c: (typeof graph.components)[number]; r: Rect; folder: string }[] = [];
		for (const f of folders) {
			const inner = { x: f.x + 3, y: f.y + 22, w: Math.max(0, f.w - 6), h: Math.max(0, f.h - 25) };
			const list = groups.get(f.id)!;
			const items = list.map((c) => ({ id: c.id, value: Math.max(1, c.lines) })).sort((a, b) => b.value - a.value);
			for (const t of squarify(items, inner)) {
				comps.push({ c: list.find((c) => c.id === t.id)!, r: t, folder: f.id });
			}
		}
		return { folders, comps };
	});

	function fill(c: (typeof graph.components)[number]): string {
		if (color === 'sensitive') {
			return c.labels.some((l) => sensitive.includes(l))
				? 'color-mix(in oklab, var(--signal) 55%, var(--card))'
				: 'color-mix(in oklab, var(--muted-foreground) 14%, var(--card))';
		}
		const t = (usedBy.get(c.id)?.size ?? 0) / maxUsed;
		return `color-mix(in oklab, var(--info) ${Math.round(10 + t * 70)}%, var(--card))`;
	}
</script>

<div class="overflow-hidden rounded-lg border bg-muted/40">
	<svg viewBox="0 0 {W} {H}" class="block h-auto w-full">
		{#each tiles.folders as f (f.id)}
			<rect x={f.x + 1} y={f.y + 1} width={Math.max(0, f.w - 2)} height={Math.max(0, f.h - 2)} rx="6" class="fill-card stroke-border" />
			{#if f.w > 60}
				<text x={f.x + 8} y={f.y + 15} class="fill-muted-foreground text-[11px] font-semibold">{f.id}/</text>
			{/if}
		{/each}
		{#each tiles.comps as t (t.c.id)}
			{@const big = t.r.w > 70 && t.r.h > 30}
			<g
				class="cursor-pointer"
				role="button"
				tabindex="0"
				aria-label={t.c.id}
				onclick={() => (selected = t.c.id)}
				ondblclick={() => onopen?.(t.c.id)}
				onkeydown={(e) => e.key === 'Enter' && (selected = t.c.id)}
			>
				<rect
					x={t.r.x + 1}
					y={t.r.y + 1}
					width={Math.max(0, t.r.w - 2)}
					height={Math.max(0, t.r.h - 2)}
					rx="3"
					fill={fill(t.c)}
					stroke={selected === t.c.id ? 'var(--foreground)' : 'var(--card)'}
					stroke-width={selected === t.c.id ? 2.5 : 1}
					class="transition-opacity hover:opacity-85"
				>
					<title>{t.c.id}: {t.c.lines.toLocaleString()} lines, {t.c.files} files, used by {usedBy.get(t.c.id)?.size ?? 0} components</title>
				</rect>
				{#if big}
					<text x={t.r.x + 6} y={t.r.y + 16} class="pointer-events-none fill-foreground text-[11px] font-medium">
						{t.c.id.length * 6.2 > t.r.w - 10 ? t.c.id.slice(0, Math.max(3, Math.floor((t.r.w - 16) / 6.2))) + '…' : t.c.id}
					</text>
					{#if t.r.h > 44}
						<text x={t.r.x + 6} y={t.r.y + 30} class="pointer-events-none fill-muted-foreground text-[10px]">{t.c.lines.toLocaleString()} lines</text>
					{/if}
				{/if}
			</g>
		{/each}
	</svg>
</div>
