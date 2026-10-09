<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Progress } from '$lib/components/ui/progress/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { api } from '#lib/api.ts';
	import { app } from '#lib/app.svelte.ts';
	import BarList from '#lib/components/BarList.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import Graph from '#lib/components/Graph.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import { ago, componentHref, short } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Graph as G, KeysStatus } from '#lib/types.ts';
	import ArrowRight from '@lucide/svelte/icons/arrow-right';
	import Boxes from '@lucide/svelte/icons/boxes';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import Circle from '@lucide/svelte/icons/circle';
	import FileCode from '@lucide/svelte/icons/file-code';
	import FlaskConical from '@lucide/svelte/icons/flask-conical';
	import GitCompareArrows from '@lucide/svelte/icons/git-compare-arrows';
	import Network from '@lucide/svelte/icons/network';
	import PencilLine from '@lucide/svelte/icons/pencil-line';
	import Braces from '@lucide/svelte/icons/braces';
	import { onMount } from 'svelte';

	const graph = new Task<G>();
	const keys = new Task<KeysStatus>();

	onMount(() => {
		graph.run(() => api<G>('map.graph'));
		keys.run(() => api<KeysStatus>('keys.status'));
		app.loadRefs();
	});

	const s = $derived(app.status);
	const setup = $derived(
		s
			? [
					{ done: s.config.exists, label: 'onus.yaml declares components, labels and rules', href: '/config', how: 'onus init' },
					{ done: s.config.lanes, label: 'A lane policy decides what merges on its own', href: '/lanes', how: 'lanes:' },
					{ done: s.config.environment, label: 'An environment runs tests and records evidence', href: '/environments', how: 'environment:' },
					{ done: !!keys.value?.public, label: 'A root key signs task tokens', href: '/scopes', how: 'onus token keygen' }
				]
			: []
	);
	const done = $derived(setup.filter((i) => i.done).length);
	const largest = $derived(
		[...(graph.value?.components ?? [])]
			.sort((a, b) => b.lines - a.lines)
			.slice(0, 8)
			.map((c) => ({ label: c.id, value: c.lines, href: componentHref(c.id), hint: `${c.files} files` }))
	);
	const connected = $derived.by(() => {
		const g = graph.value;
		if (!g) return [];
		const used = new Map<string, number>();
		for (const e of g.edges) used.set(e.to, (used.get(e.to) ?? 0) + 1);
		return [...used.entries()]
			.sort((a, b) => b[1] - a[1])
			.slice(0, 8)
			.map(([id, n]) => ({ label: id, value: n, href: componentHref(id) }));
	});
</script>

<PageHead title="Overview">What Onus knows about this repository, and where to go next.</PageHead>

{#if !s}
	<div class="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
		{#each Array(6) as _, i (i)}<Skeleton class="h-24 rounded-xl" />{/each}
	</div>
	<p class="text-sm text-muted-foreground">Building the map of the repository…</p>
{:else}
	<div class="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
		<Stat label="Components" value={s.map.components} icon={Boxes} href="/map?tab=components" hint="{Object.keys(s.map.componentDirs).length} folders" />
		<Stat label="Files" value={s.map.files} icon={FileCode} href="/map?tab=files" />
		<Stat label="Symbols" value={s.map.symbols} icon={Braces} href="/map?tab=find" />
		<Stat label="Relationships" value={s.map.edges} icon={Network} href="/map" />
		<Stat label="Tests" value={s.map.tests} icon={FlaskConical} />
		<Stat
			label="Uncommitted files"
			value={s.dirty.length}
			icon={PencilLine}
			href="/changes"
			hint={s.dirty.length ? 'See what they mean' : 'The working tree is clean'}
			tone={s.dirty.length ? 'signal' : undefined}
		/>
	</div>

	<div class="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
		<Card title="Components" subtitle="Who depends on whom. Click one to open it.">
			{#snippet actions()}<Button variant="outline" size="sm" href="/map">Open the map<ArrowRight /></Button>{/snippet}
			{#if graph.value}
				{#if graph.value.components.length}
					<Graph graph={graph.value} limit={18} />
				{:else}
					<p class="text-sm text-muted-foreground">The map has no components yet. Onus reads TypeScript and JavaScript; other languages come in through plugins.</p>
				{/if}
			{:else if graph.error}
				<p class="text-sm text-muted-foreground">{graph.error}</p>
			{:else}
				<Skeleton class="h-72 w-full" />
			{/if}
		</Card>
		<div class="grid content-start gap-4">
			<Card title="Largest components" subtitle="Lines of code">
				{#if graph.value}<BarList items={largest} />{:else}<Skeleton class="h-48" />{/if}
			</Card>
			<Card title="Most depended on" subtitle="Components that use them">
				{#if graph.value}<BarList items={connected} />{:else}<Skeleton class="h-48" />{/if}
			</Card>
		</div>
	</div>

	<div class="grid gap-4 lg:grid-cols-3">
		<Card title="Recent commits" subtitle="Report what any commit meant." class="lg:col-span-2" pad={false}>
			{#snippet actions()}<Button variant="outline" size="sm" href="/changes"><GitCompareArrows />Compare refs</Button>{/snippet}
			<ul class="divide-y">
				{#each (app.refs?.commits ?? []).slice(0, 8) as c (c.sha)}
					<li class="group flex items-center gap-3 px-4 py-2.5 text-sm hover:bg-muted/40">
						<span class="font-mono text-xs text-muted-foreground">{short(c.sha, 7)}</span>
						<span class="min-w-0 flex-1 truncate" title={c.subject}>{c.subject}</span>
						<span class="hidden shrink-0 text-xs text-muted-foreground sm:inline">{ago(c.at)}</span>
						<Button variant="ghost" size="xs" href="/changes?base={c.sha}~1&head={c.sha}" class="opacity-60 group-hover:opacity-100">Report<ArrowRight /></Button>
					</li>
				{:else}
					<li class="px-4 py-3 text-sm text-muted-foreground">No commits yet.</li>
				{/each}
			</ul>
		</Card>
		<div class="grid content-start gap-4">
			<Card title="Set up" subtitle="{done} of {setup.length} done">
				<Progress value={(done / Math.max(1, setup.length)) * 100} class="mb-4 h-1.5" />
				<ul class="grid gap-2.5 text-sm">
					{#each setup as item (item.label)}
						<li class="flex items-start gap-2">
							{#if item.done}<CircleCheck class="mt-0.5 size-4 shrink-0 text-success" />{:else}<Circle class="mt-0.5 size-4 shrink-0 text-muted-foreground" />{/if}
							<div class="grid gap-0.5">
								<a href={item.href} class="hover:underline {item.done ? 'text-muted-foreground' : ''}">{item.label}</a>
								{#if !item.done}<code class="w-fit text-xs">{item.how}</code>{/if}
							</div>
						</li>
					{/each}
				</ul>
			</Card>
			<Card title="Connect an agent" subtitle="Agents check their work and see what a change would break, over MCP.">
				<div class="flex items-center gap-2">
					<code class="flex-1 truncate rounded-md bg-muted px-2.5 py-1.5 text-xs">claude mcp add onus -- onus mcp</code>
					<Copy text="claude mcp add onus -- onus mcp" />
				</div>
				<p class="mt-2 text-xs text-muted-foreground">Any MCP client works, on stdio or <code>onus mcp --http 127.0.0.1:8765</code>. See <a class="underline" href="/guide/agents">agents</a>.</p>
			</Card>
		</div>
	</div>
{/if}
