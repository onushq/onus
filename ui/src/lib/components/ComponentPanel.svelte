<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { query } from '#lib/api.ts';
	import { componentHref, symbolHref } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { ComponentInfo, Tests } from '#lib/types.ts';
	import ArrowRight from '@lucide/svelte/icons/arrow-right';
	import Crosshair from '@lucide/svelte/icons/crosshair';
	import X from '@lucide/svelte/icons/x';
	import Zap from '@lucide/svelte/icons/zap';
	import Badge from './Badge.svelte';

	// What a selected component is, without leaving the map.
	let {
		id,
		onselect,
		onfocus,
		onclose
	}: { id: string; onselect: (id: string) => void; onfocus: (id: string) => void; onclose: () => void } = $props();

	const info = new Task<ComponentInfo>();
	const tests = new Task<Tests>();

	$effect(() => {
		const c = id;
		info.run(() => query<ComponentInfo>({ query: 'component', id: c }));
		tests.run(() => query<Tests>({ query: 'tests-for', target: c, limit: 1 }));
	});

	const sorted = (m: Record<string, number>) => Object.entries(m).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
</script>

<aside class="flex max-h-[70vh] min-w-0 flex-col overflow-hidden rounded-xl border bg-card shadow-xs">
	<div class="flex items-start justify-between gap-2 border-b px-4 py-3">
		<div class="grid min-w-0 gap-0.5">
			<span class="truncate font-semibold">{id}</span>
			{#if info.value}<span class="truncate font-mono text-xs text-muted-foreground">{info.value.roots.join(', ')}</span>{/if}
		</div>
		<Button variant="ghost" size="icon-sm" aria-label="Close" onclick={onclose}><X /></Button>
	</div>
	<div class="grid gap-4 overflow-y-auto px-4 py-3 text-sm">
		{#if info.error}
			<p class="text-destructive">{info.error}</p>
		{:else if !info.value}
			<Skeleton class="h-40" />
		{:else}
			{@const c = info.value}
			<div class="grid grid-cols-3 gap-2 text-center">
				<div class="rounded-lg bg-muted/60 py-2"><div class="text-lg font-semibold tabular-nums">{c.files}</div><div class="text-[11px] text-muted-foreground">files</div></div>
				<div class="rounded-lg bg-muted/60 py-2"><div class="text-lg font-semibold tabular-nums">{c.publicSymbols}</div><div class="text-[11px] text-muted-foreground">public</div></div>
				<div class="rounded-lg bg-muted/60 py-2"><div class="text-lg font-semibold tabular-nums">{tests.value?.total ?? '…'}</div><div class="text-[11px] text-muted-foreground">test files</div></div>
			</div>
			<div class="flex flex-wrap gap-1">
				<Badge tone="faint">{c.kind}</Badge>
				{#each c.labels as l (l)}<Badge tone="signal">{l}</Badge>{/each}
				{#each c.owners as o (o)}<Badge>{o}</Badge>{/each}
			</div>
			<div class="flex flex-wrap gap-2">
				<Button size="sm" href={componentHref(id)}>Open<ArrowRight /></Button>
				<Button size="sm" variant="outline" onclick={() => onfocus(id)}><Crosshair />Focus</Button>
				<Button size="sm" variant="outline" href="/map/symbol?id={encodeURIComponent(id)}&impact=1"><Zap />Impact</Button>
			</div>
			{#each [['Uses', c.uses, 'text-info'], ['Used by', c.usedBy, 'text-signal-foreground']] as [label, map, cls] (label)}
				{@const list = sorted(map as Record<string, number>)}
				<div class="grid gap-1.5">
					<span class="text-xs font-medium text-muted-foreground">{label} · {list.length}</span>
					{#if list.length}
						<ul class="grid gap-0.5">
							{#each list.slice(0, 12) as [other, n] (other)}
								<li>
									<button class="flex w-full items-center gap-2 rounded px-1.5 py-1 text-left hover:bg-muted" onclick={() => onselect(other)}>
										<span class="truncate {cls}">{other}</span>
										<span class="ml-auto font-mono text-[11px] text-muted-foreground">{n}</span>
									</button>
								</li>
							{/each}
							{#if list.length > 12}<li class="px-1.5 text-xs text-muted-foreground">and {list.length - 12} more</li>{/if}
						</ul>
					{:else}<span class="text-xs text-muted-foreground">Nothing</span>{/if}
				</div>
			{/each}
			{#if c.externals.length || c.eventsPublished.length || c.eventsConsumed.length}
				<div class="grid gap-1 text-xs">
					{#if c.externals.length}<span><span class="text-muted-foreground">Externals:</span> {c.externals.join(', ')}</span>{/if}
					{#if c.eventsPublished.length}<span><span class="text-muted-foreground">Publishes:</span> <span class="font-mono">{c.eventsPublished.join(', ')}</span></span>{/if}
					{#if c.eventsConsumed.length}<span><span class="text-muted-foreground">Consumes:</span> <span class="font-mono">{c.eventsConsumed.join(', ')}</span></span>{/if}
				</div>
			{/if}
			{#if c.public.length}
				<div class="grid gap-1">
					<span class="text-xs font-medium text-muted-foreground">Public surface</span>
					<ul class="grid gap-0.5">
						{#each c.public.slice(0, 8) as s (s.id)}
							<li class="flex items-center gap-2"><a class="truncate hover:underline" href={symbolHref(s.id)}>{s.name}</a><span class="text-[11px] text-muted-foreground">{s.kind}</span></li>
						{/each}
					</ul>
				</div>
			{/if}
		{/if}
	</div>
</aside>
