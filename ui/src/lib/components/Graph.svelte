<script lang="ts">
	import { goto } from '$app/navigation';
	import { componentHref } from '#lib/format.ts';
	import type { Graph } from '#lib/types.ts';

	// Components in layers: what nothing else in the map depends on at the
	// top, foundations at the bottom. The layout is computed, not simulated,
	// so the same map always draws the same picture.
	let {
		graph,
		sensitive = [],
		limit = 40,
		focus = ''
	}: { graph: Graph; sensitive?: string[]; limit?: number; focus?: string } = $props();

	const W = 168;
	const H = 52;
	const GAP_X = 36;
	const GAP_Y = 84;
	const PAD = 24;

	let hover = $state<string | null>(null);
	/** The most components a row holds before it wraps. */
	const ROW = 7;

	// What is drawn: a component and its neighbours when focused, else the
	// most connected components up to the limit. Large maps stay readable.
	const shown = $derived.by(() => {
		const degree = new Map<string, number>();
		for (const e of graph.edges) {
			degree.set(e.from, (degree.get(e.from) ?? 0) + e.count);
			degree.set(e.to, (degree.get(e.to) ?? 0) + e.count);
		}
		const byDegree = (a: string, b: string) => (degree.get(b) ?? 0) - (degree.get(a) ?? 0) || a.localeCompare(b);
		let ids: string[];
		if (focus && graph.components.some((c) => c.id === focus)) {
			const near = new Set<string>();
			for (const e of graph.edges) {
				if (e.from === focus) near.add(e.to);
				if (e.to === focus) near.add(e.from);
			}
			ids = [focus, ...[...near].sort(byDegree).slice(0, Math.max(0, limit - 1))];
		} else {
			ids = graph.components.map((c) => c.id).sort(byDegree).slice(0, limit);
		}
		const keep = new Set(ids);
		return {
			components: graph.components.filter((c) => keep.has(c.id)),
			edges: graph.edges.filter((e) => keep.has(e.from) && keep.has(e.to)),
			hidden: focus ? 0 : graph.components.length - keep.size
		};
	});

	const layout = $derived.by(() => {
		const ids = shown.components.map((c) => c.id).sort();
		const out = new Map<string, string[]>();
		for (const id of ids) out.set(id, []);
		for (const e of shown.edges) {
			if (out.has(e.from) && out.has(e.to)) out.get(e.from)!.push(e.to);
		}
		// Layer = longest path to a component with no dependencies; edges
		// that close a cycle are ignored.
		const layer = new Map<string, number>();
		const visiting = new Set<string>();
		const depth = (id: string): number => {
			if (layer.has(id)) return layer.get(id)!;
			if (visiting.has(id)) return 0;
			visiting.add(id);
			let d = 0;
			for (const to of out.get(id) ?? []) {
				if (!visiting.has(to)) d = Math.max(d, depth(to) + 1);
			}
			visiting.delete(id);
			layer.set(id, d);
			return d;
		};
		ids.forEach(depth);
		const max = Math.max(0, ...layer.values());
		const rows: string[][] = Array.from({ length: max + 1 }, () => []);
		for (const id of ids) rows[max - layer.get(id)!].push(id);
		// Order each row by where its neighbours sit, a few sweeps.
		const pos = new Map<string, number>();
		rows.forEach((r) => r.forEach((id, i) => pos.set(id, i)));
		const neighbours = new Map<string, string[]>();
		for (const id of ids) neighbours.set(id, []);
		for (const e of shown.edges) {
			neighbours.get(e.from)?.push(e.to);
			neighbours.get(e.to)?.push(e.from);
		}
		for (let sweep = 0; sweep < 4; sweep++) {
			for (const r of rows) {
				const score = (id: string) => {
					const n = neighbours.get(id)!.filter((x) => pos.has(x));
					return n.length ? n.reduce((s, x) => s + pos.get(x)!, 0) / n.length : pos.get(id)!;
				};
				const scored = r.map((id) => [id, score(id)] as const);
				scored.sort((a, b) => a[1] - b[1] || a[0].localeCompare(b[0]));
				scored.forEach(([id], i) => {
					r[i] = id;
					pos.set(id, i);
				});
			}
		}
		// Long layers wrap into several rows.
		const wrapped = rows.flatMap((r) => {
			const out: string[][] = [];
			for (let i = 0; i < r.length; i += ROW) out.push(r.slice(i, i + ROW));
			return out.length ? out : [r];
		});
		rows.length = 0;
		rows.push(...wrapped);
		const widest = Math.max(1, ...rows.map((r) => r.length));
		const width = PAD * 2 + widest * W + (widest - 1) * GAP_X;
		const height = PAD * 2 + rows.length * H + (rows.length - 1) * GAP_Y;
		const xy = new Map<string, { x: number; y: number }>();
		rows.forEach((r, ri) => {
			const rowWidth = r.length * W + (r.length - 1) * GAP_X;
			const x0 = (width - rowWidth) / 2;
			r.forEach((id, i) => xy.set(id, { x: x0 + i * (W + GAP_X), y: PAD + ri * (H + GAP_Y) }));
		});
		return { xy, width, height };
	});

	const byId = $derived(new Map(shown.components.map((c) => [c.id, c])));
	const maxCount = $derived(Math.max(1, ...shown.edges.map((e) => e.count)));

	// Where each edge leaves and enters a node: spread along the node's
	// bottom and top, ordered by where the other end sits, so arrows do not
	// pile onto one point.
	const ports = $derived.by(() => {
		const out = new Map<string, number>();
		const into = new Map<string, number>();
		const spread = (key: (e: { from: string; to: string }) => string, other: (e: { from: string; to: string }) => string, target: Map<string, number>) => {
			const groups = new Map<string, { from: string; to: string }[]>();
			for (const e of shown.edges) {
				if (!layout.xy.has(e.from) || !layout.xy.has(e.to)) continue;
				const k = key(e);
				if (!groups.has(k)) groups.set(k, []);
				groups.get(k)!.push(e);
			}
			for (const list of groups.values()) {
				list.sort((a, b) => layout.xy.get(other(a))!.x - layout.xy.get(other(b))!.x);
				list.forEach((e, i) => target.set(e.from + '>' + e.to, ((i + 1) / (list.length + 1)) * W));
			}
		};
		spread((e) => e.from, (e) => e.to, out);
		spread((e) => e.to, (e) => e.from, into);
		return { out, into };
	});

	function path(from: string, to: string): string {
		const a = layout.xy.get(from)!;
		const b = layout.xy.get(to)!;
		const key = from + '>' + to;
		const sx = a.x + (ports.out.get(key) ?? W / 2);
		const tx = b.x + (ports.into.get(key) ?? W / 2);
		if (b.y > a.y) {
			const sy = a.y + H;
			const ty = b.y;
			const my = (sy + ty) / 2;
			return `M${sx},${sy} C${sx},${my} ${tx},${my} ${tx},${ty}`;
		}
		// Upward or sideways (a cycle or a same-row edge): around the side.
		const sy = a.y + H / 2;
		const ty = b.y + H / 2;
		const side = Math.max(a.x, b.x) + W + 18;
		return `M${a.x + W},${sy} C${side},${sy} ${side},${ty} ${b.x + W},${ty}`;
	}

	const touches = (e: { from: string; to: string }) => hover === e.from || hover === e.to;
</script>

{#if shown.hidden > 0 || focus}
	<p class="note">
		{#if focus}{focus} and the {shown.components.length - 1} components it is directly connected to{:else}The {shown.components.length} most connected of {graph.components.length} components{/if}
	</p>
{/if}
<div class="wrap">
	<svg
		style="width: 100%; max-width: {layout.width}px; height: auto"
		viewBox="0 0 {layout.width} {layout.height}"
		role="img"
		aria-label="How the components depend on each other"
	>
		<defs>
			<marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
				<path d="M0,0 L10,5 L0,10 z" fill="var(--ink-faint)" />
			</marker>
			<marker id="arrow-hot" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
				<path d="M0,0 L10,5 L0,10 z" fill="var(--ink)" />
			</marker>
		</defs>
		{#each shown.edges as e (e.from + '>' + e.to)}
			{#if layout.xy.has(e.from) && layout.xy.has(e.to)}
				<path
					d={path(e.from, e.to)}
					class="edge"
					class:hot={touches(e)}
					class:dim={hover && !touches(e)}
					stroke-width={1 + 2.5 * Math.log1p(e.count) / Math.log1p(maxCount)}
					marker-end={touches(e) ? 'url(#arrow-hot)' : 'url(#arrow)'}
				>
					<title>{e.from} uses {e.to}: {Object.entries(e.kinds).map(([k, n]) => `${n} ${k}`).join(', ')}</title>
				</path>
			{/if}
		{/each}
		{#each shown.components as c (c.id)}
			{@const p = layout.xy.get(c.id)!}
			{@const hot = c.labels.some((l) => sensitive.includes(l))}
			<g
				class="node"
				class:dim={hover && hover !== c.id && !shown.edges.some((e) => (e.from === hover && e.to === c.id) || (e.to === hover && e.from === c.id))}
				transform="translate({p.x},{p.y})"
				role="link"
				tabindex="0"
				onmouseenter={() => (hover = c.id)}
				onmouseleave={() => (hover = null)}
				onfocus={() => (hover = c.id)}
				onblur={() => (hover = null)}
				onclick={() => goto(componentHref(c.id))}
				onkeydown={(ev) => ev.key === 'Enter' && goto(componentHref(c.id))}
			>
				<rect width={W} height={H} rx="10" class:hot />
				<text x="12" y="21" class="name">{c.id.length > 20 ? c.id.slice(0, 19) + '…' : c.id}</text>
				<text x="12" y="39" class="meta">{c.kind} · {c.files} files{c.labels.length ? ' · ' + c.labels.join(', ') : ''}</text>
				<title>{c.id} ({c.kind}){c.owners.length ? `, owned by ${c.owners.join(', ')}` : ''}</title>
			</g>
		{/each}
	</svg>
</div>

<style>
	.note {
		font-size: 12px;
		color: var(--ink-muted);
		margin: 0 0 var(--space-2);
	}
	.wrap {
		overflow: auto;
		max-height: 70vh;
		background:
			radial-gradient(circle, var(--line) 1px, transparent 1px) 0 0 / 18px 18px,
			var(--sunken);
		border-radius: var(--radius-s);
	}
	svg {
		display: block;
		margin: 0 auto;
	}
	.edge {
		fill: none;
		stroke: var(--ink-faint);
		opacity: 0.7;
		transition: opacity 0.15s;
	}
	.edge.hot {
		stroke: var(--ink);
		opacity: 1;
	}
	.edge.dim {
		opacity: 0.12;
	}
	.node {
		cursor: pointer;
		transition: opacity 0.15s;
	}
	.node.dim {
		opacity: 0.35;
	}
	.node:focus {
		outline: none;
	}
	rect {
		fill: var(--surface);
		stroke: var(--line);
		stroke-width: 1.5;
	}
	rect.hot {
		stroke: var(--signal);
	}
	.node:hover rect,
	.node:focus-visible rect {
		stroke: var(--ink);
	}
	.name {
		font-weight: 650;
		font-size: 13px;
		fill: var(--ink);
	}
	.meta {
		font-size: 11px;
		fill: var(--ink-muted);
	}
</style>
