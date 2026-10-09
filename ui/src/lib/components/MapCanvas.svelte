<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import { byFolder, edgePath, layered, NODE_H, NODE_W, type Layout } from '#lib/graph/layout.ts';
	import type { Graph } from '#lib/types.ts';
	import Maximize from '@lucide/svelte/icons/maximize';
	import Minus from '@lucide/svelte/icons/minus';
	import Plus from '@lucide/svelte/icons/plus';
	import { untrack } from 'svelte';

	// The component map as a canvas: pan by dragging, zoom with the wheel or
	// the buttons. A click selects (the page shows details), a double click
	// opens the component. Edges a selected component uses are blue, edges
	// that use it are amber.
	let {
		graph,
		sensitive = [],
		layout: mode = 'layers',
		kinds,
		focus = '',
		hops = 1,
		limit = 60,
		selected = $bindable(''),
		onopen,
		height = '70vh'
	}: {
		graph: Graph;
		sensitive?: string[];
		layout?: 'layers' | 'folders';
		kinds: Set<string>;
		focus?: string;
		hops?: number;
		limit?: number;
		selected?: string;
		onopen?: (id: string) => void;
		height?: string;
	} = $props();

	// Edges of the shown kinds, weighted by those kinds only.
	const edges = $derived(
		graph.edges
			.map((e) => {
				const count = Object.entries(e.kinds).reduce((s, [k, n]) => s + (kinds.has(k) ? n : 0), 0);
				return { ...e, count };
			})
			.filter((e) => e.count > 0)
	);

	const shown = $derived.by(() => {
		const degree = new Map<string, number>();
		for (const e of edges) {
			degree.set(e.from, (degree.get(e.from) ?? 0) + e.count);
			degree.set(e.to, (degree.get(e.to) ?? 0) + e.count);
		}
		const byDegree = (a: string, b: string) => (degree.get(b) ?? 0) - (degree.get(a) ?? 0) || a.localeCompare(b);
		const exists = new Set(graph.components.map((c) => c.id));
		let ids: string[];
		if (focus && exists.has(focus)) {
			const keep = new Set([focus]);
			let frontier = [focus];
			for (let h = 0; h < hops; h++) {
				const next: string[] = [];
				for (const e of edges) {
					for (const [a, b] of [[e.from, e.to], [e.to, e.from]]) {
						if (frontier.includes(a) && !keep.has(b)) {
							keep.add(b);
							next.push(b);
						}
					}
				}
				frontier = next;
			}
			ids = [focus, ...[...keep].filter((i) => i !== focus).sort(byDegree)].slice(0, limit);
		} else {
			ids = [...exists].sort(byDegree).slice(0, limit);
		}
		const keep = new Set(ids);
		return {
			components: graph.components.filter((c) => keep.has(c.id)),
			edges: edges.filter((e) => keep.has(e.from) && keep.has(e.to)),
			hidden: exists.size - keep.size
		};
	});

	const placed: Layout = $derived(
		mode === 'folders' ? byFolder(shown.components, shown.edges) : layered(shown.components, shown.edges)
	);
	const maxCount = $derived(Math.max(1, ...shown.edges.map((e) => e.count)));

	// Ports: edges leave and enter spread along a node's width.
	const ports = $derived.by(() => {
		const out = new Map<string, number>();
		const into = new Map<string, number>();
		const spread = (key: (e: { from: string; to: string }) => string, other: (e: { from: string; to: string }) => string, target: Map<string, number>) => {
			const groups = new Map<string, { from: string; to: string }[]>();
			for (const e of shown.edges) {
				if (!placed.xy.has(e.from) || !placed.xy.has(e.to)) continue;
				const k = key(e);
				if (!groups.has(k)) groups.set(k, []);
				groups.get(k)!.push(e);
			}
			for (const list of groups.values()) {
				list.sort((a, b) => placed.xy.get(other(a))!.x - placed.xy.get(other(b))!.x);
				list.forEach((e, i) => target.set(e.from + '>' + e.to, ((i + 1) / (list.length + 1)) * NODE_W));
			}
		};
		spread((e) => e.from, (e) => e.to, out);
		spread((e) => e.to, (e) => e.from, into);
		return { out, into };
	});

	// --- Pan and zoom

	let box: HTMLDivElement | undefined = $state();
	let view = $state({ x: 0, y: 0, k: 1 });
	let size = $state({ w: 800, h: 500 });

	/**
	 * Fits the drawing. When everything at once would be too small to read,
	 * the first view stays readable instead: the top of the drawing, or the
	 * focused component, at a zoom where names can be read. The fit button
	 * always shows everything.
	 */
	function fit(readable = false) {
		if (!box) return;
		const w = box.clientWidth;
		const h = box.clientHeight;
		size = { w, h };
		const k = Math.min(1.25, Math.max(0.05, Math.min(w / placed.width, h / placed.height) * 0.95));
		const MIN = 0.55;
		if (!readable || k >= MIN) {
			view = { k, x: (w - placed.width * k) / 2, y: (h - placed.height * k) / 2 };
			return;
		}
		const at = (focus && placed.xy.get(focus)) || (selected && placed.xy.get(selected));
		if (at) {
			view = { k: MIN, x: w / 2 - (at.x + NODE_W / 2) * MIN, y: h / 2 - (at.y + NODE_H / 2) * MIN };
		} else {
			view = { k: MIN, x: Math.min(24, (w - placed.width * MIN) / 2), y: 24 };
		}
	}

	// Refit when what is drawn changes.
	$effect(() => {
		void placed;
		untrack(() => requestAnimationFrame(() => fit(true)));
	});

	function zoomAt(factor: number, cx = size.w / 2, cy = size.h / 2) {
		const k = Math.min(3, Math.max(0.05, view.k * factor));
		const f = k / view.k;
		view = { k, x: cx - (cx - view.x) * f, y: cy - (cy - view.y) * f };
	}

	function wheel(e: WheelEvent) {
		e.preventDefault();
		const r = box!.getBoundingClientRect();
		if (e.ctrlKey || e.metaKey || Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
			zoomAt(Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.002)), e.clientX - r.left, e.clientY - r.top);
		} else {
			view = { ...view, x: view.x - e.deltaX, y: view.y - e.deltaY };
		}
	}

	let drag = $state<{ x: number; y: number; vx: number; vy: number; moved: boolean } | null>(null);
	function down(e: PointerEvent) {
		if (e.button !== 0) return;
		drag = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y, moved: false };
		(e.currentTarget as Element).setPointerCapture(e.pointerId);
	}
	function move(e: PointerEvent) {
		if (!drag) return;
		const dx = e.clientX - drag.x;
		const dy = e.clientY - drag.y;
		if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
		view = { ...view, x: drag.vx + dx, y: drag.vy + dy };
	}
	function up(e: PointerEvent) {
		// The pointer is captured, so find what is under it.
		const target = document.elementFromPoint(e.clientX, e.clientY)?.closest('[data-node]');
		if (drag && !drag.moved) selected = target ? target.getAttribute('data-node')! : '';
		drag = null;
	}

	let hover = $state('');
	const active = $derived(hover || selected);
	const related = $derived.by(() => {
		const r = new Set<string>();
		if (!active) return r;
		for (const e of shown.edges) {
			if (e.from === active) r.add(e.to);
			if (e.to === active) r.add(e.from);
		}
		return r;
	});
	const byId = $derived(new Map(graph.components.map((c) => [c.id, c])));
</script>

<div class="relative overflow-hidden rounded-lg border bg-muted/40" style="height: {height}">
	<div
		bind:this={box}
		class="size-full touch-none select-none {drag ? 'cursor-grabbing' : 'cursor-grab'}"
		role="application"
		aria-label="Component map: drag to pan, scroll to zoom, click a component to select it"
		onwheel={wheel}
		onpointerdown={down}
		onpointermove={move}
		onpointerup={up}
		ondblclick={(e) => {
			const t = (e.target as Element).closest('[data-node]');
			if (t) onopen?.(t.getAttribute('data-node')!);
		}}
	>
		<svg width="100%" height="100%" class="block">
			<defs>
				<pattern id="dots" width="20" height="20" patternUnits="userSpaceOnUse" patternTransform="translate({view.x} {view.y}) scale({view.k})">
					<circle cx="1" cy="1" r="1" fill="var(--border)" />
				</pattern>
				{#each [['a-faint', 'var(--faint)'], ['a-out', 'var(--info)'], ['a-in', 'var(--signal)']] as [id, color] (id)}
					<marker {id} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
						<path d="M0,0 L10,5 L0,10 z" fill={color} />
					</marker>
				{/each}
			</defs>
			<rect width="100%" height="100%" fill="url(#dots)" />
			<g transform="translate({view.x} {view.y}) scale({view.k})">
				{#each placed.groups as g (g.name)}
					<rect x={g.x} y={g.y} width={g.w} height={g.h} rx="14" class="fill-card/70 stroke-border" stroke-width="1.5" />
					<text x={g.x + 14} y={g.y + 22} class="fill-muted-foreground text-[12px] font-semibold">{g.name}/ <tspan class="font-normal">· {g.count}</tspan></text>
				{/each}
				{#each shown.edges as e (e.from + '>' + e.to)}
					{#if placed.xy.has(e.from) && placed.xy.has(e.to)}
						{@const out = active && e.from === active}
						{@const inn = active && e.to === active}
						<path
							d={edgePath(placed.xy.get(e.from)!, placed.xy.get(e.to)!, ports.out.get(e.from + '>' + e.to), ports.into.get(e.from + '>' + e.to))}
							fill="none"
							stroke={out ? 'var(--info)' : inn ? 'var(--signal)' : 'var(--faint)'}
							stroke-width={(1 + (2.5 * Math.log1p(e.count)) / Math.log1p(maxCount)) / Math.max(0.6, view.k)}
							opacity={active ? (out || inn ? 1 : 0.08) : 0.55}
							marker-end={out ? 'url(#a-out)' : inn ? 'url(#a-in)' : 'url(#a-faint)'}
							class="transition-opacity"
						>
							<title>{e.from} uses {e.to}: {Object.entries(e.kinds).map(([k, n]) => `${n} ${k}`).join(', ')}</title>
						</path>
					{/if}
				{/each}
				{#each shown.components as c (c.id)}
					{@const p = placed.xy.get(c.id)}
					{#if p}
						{@const hot = c.labels.some((l) => sensitive.includes(l))}
						{@const dim = active && active !== c.id && !related.has(c.id)}
						<g
							data-node={c.id}
							transform="translate({p.x},{p.y})"
							class="cursor-pointer transition-opacity"
							opacity={dim ? 0.3 : 1}
							role="button"
							tabindex="0"
							aria-label={c.id}
							onpointerenter={() => (hover = c.id)}
							onpointerleave={() => (hover = '')}
							onkeydown={(ev) => ev.key === 'Enter' && (selected = c.id)}
						>
							<rect
								width={NODE_W}
								height={NODE_H}
								rx="10"
								class="fill-card"
								stroke={selected === c.id ? 'var(--foreground)' : hot ? 'var(--signal)' : 'var(--border)'}
								stroke-width={selected === c.id ? 2.5 : 1.5}
							/>
							<text x="12" y="22" class="fill-foreground text-[13px] font-semibold">{c.id.length > 21 ? c.id.slice(0, 20) + '…' : c.id}</text>
							<text x="12" y="40" class="fill-muted-foreground text-[11px]">{c.kind} · {c.files} files{c.labels.length ? ' · ' + c.labels.join(', ') : ''}</text>
						</g>
					{/if}
				{/each}
			</g>
		</svg>
	</div>

	<div class="absolute top-3 right-3 flex flex-col gap-1 rounded-lg border bg-background/90 p-1 shadow-sm backdrop-blur">
		<Tooltip.Root>
			<Tooltip.Trigger>{#snippet child({ props })}<Button {...props} variant="ghost" size="icon-sm" aria-label="Zoom in" onclick={() => zoomAt(1.25)}><Plus /></Button>{/snippet}</Tooltip.Trigger>
			<Tooltip.Content side="left">Zoom in</Tooltip.Content>
		</Tooltip.Root>
		<Tooltip.Root>
			<Tooltip.Trigger>{#snippet child({ props })}<Button {...props} variant="ghost" size="icon-sm" aria-label="Zoom out" onclick={() => zoomAt(0.8)}><Minus /></Button>{/snippet}</Tooltip.Trigger>
			<Tooltip.Content side="left">Zoom out</Tooltip.Content>
		</Tooltip.Root>
		<Tooltip.Root>
			<Tooltip.Trigger>{#snippet child({ props })}<Button {...props} variant="ghost" size="icon-sm" aria-label="Fit" onclick={() => fit()}><Maximize /></Button>{/snippet}</Tooltip.Trigger>
			<Tooltip.Content side="left">Fit to screen</Tooltip.Content>
		</Tooltip.Root>
	</div>

	<div class="pointer-events-none absolute bottom-3 left-3 flex flex-wrap items-center gap-3 rounded-lg border bg-background/90 px-3 py-1.5 text-xs text-muted-foreground shadow-sm backdrop-blur">
		<span>{shown.components.length} of {graph.components.length} components</span>
		{#if active}
			<span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-info"></span>{byId.get(active)?.id} uses</span>
			<span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-signal"></span>uses {byId.get(active)?.id}</span>
		{:else}
			<span>drag to pan · scroll to zoom · click to inspect · double-click to open</span>
		{/if}
		<span>{Math.round(view.k * 100)}%</span>
	</div>
</div>
