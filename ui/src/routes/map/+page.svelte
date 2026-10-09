<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import { api, query } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import ComponentPanel from '#lib/components/ComponentPanel.svelte';
	import MapCanvas from '#lib/components/MapCanvas.svelte';
	import Treemap from '#lib/components/Treemap.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import NativeSelect from '#lib/components/NativeSelect.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { componentHref, fileHref, idHref, symbolHref } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { FileRow, Found, Graph as G } from '#lib/types.ts';
	import Search from '@lucide/svelte/icons/search';
	import X from '@lucide/svelte/icons/x';
	import { onMount } from 'svelte';

	const graph = new Task<G>();
	const files = new Task<{ files: FileRow[] }>();
	const found = new Task<Found>();

	let tab = $state(page.url.searchParams.get('tab') ?? (page.url.searchParams.get('q') ? 'find' : 'graph'));
	let text = $state(page.url.searchParams.get('q') ?? '');
	let fileFilter = $state('');
	let componentFilter = $state('');
	let componentText = $state('');
	let tests = $state<'all' | 'code' | 'tests'>('all');
	let focus = $state(page.url.searchParams.get('focus') ?? '');
	let limit = $state(60);
	let hops = $state(1);
	let layout = $state<'layers' | 'folders'>('layers');
	let treeColor = $state<'dependents' | 'sensitive'>('dependents');
	let selected = $state(page.url.searchParams.get('select') ?? '');
	const edgeKinds = [
		{ id: 'imports', label: 'imports' },
		{ id: 'calls', label: 'calls' },
		{ id: 'references-type', label: 'types' }
	];
	let kinds = $state(new Set(['imports', 'calls', 'references-type', 'reads', 'writes']));
	function toggleKind(k: string) {
		const next = new Set(kinds);
		if (next.has(k)) next.delete(k);
		else next.add(k);
		kinds = next;
	}

	onMount(() => {
		graph.run(() => api<G>('map.graph'));
	});

	$effect(() => {
		if (tab === 'files' && !files.value && !files.running) files.run(() => api('map.files'));
	});

	$effect(() => {
		const q = page.url.searchParams.get('q');
		if (q !== null) {
			text = q;
			tab = 'find';
			found.run(() => query<Found>({ query: 'find', text: q, limit: 100 }));
		}
	});

	function find(e: SubmitEvent) {
		e.preventDefault();
		if (text.trim()) goto(`/map?q=${encodeURIComponent(text.trim())}`, { replace: true, reset: false });
	}

	const sensitive = ['auth', 'payments', 'pii'];
	const shownFiles = $derived(
		(files.value?.files ?? []).filter(
			(f) =>
				(!fileFilter || f.path.toLowerCase().includes(fileFilter.toLowerCase())) &&
				(!componentFilter || f.component === componentFilter) &&
				(tests === 'all' || (tests === 'tests') === f.isTest)
		)
	);
	const shownComponents = $derived(
		(graph.value?.components ?? []).filter((c) => !componentText || c.id.toLowerCase().includes(componentText.toLowerCase()))
	);
</script>

<PageHead title="Map" guide="how-it-works">
	The components, contracts and relationships Onus reads from the code and onus.yaml. It follows the files on disk as you edit.
</PageHead>

<div class="flex flex-wrap items-center justify-between gap-3">
	<Tabs
		bind:value={tab}
		tabs={[
			{ id: 'graph', label: 'Graph' },
			{ id: 'treemap', label: 'Treemap' },
			{ id: 'components', label: 'Components', count: graph.value?.components.length },
			{ id: 'files', label: 'Files' },
			{ id: 'events', label: 'Events & externals', count: graph.value ? graph.value.events.length + graph.value.externals.length : undefined },
			{ id: 'find', label: 'Find' }
		]}
	/>
</div>

{#if graph.error}<ErrorBox error={graph.error} />{/if}

{#if tab === 'graph' || tab === 'treemap'}
	{#if graph.value}
		{#if graph.value.components.length}
			<Card pad={false}>
				<div class="flex flex-wrap items-center gap-2 border-b px-4 py-3">
					{#if tab === 'graph'}
						<div class="inline-flex rounded-lg bg-muted p-0.5 text-sm">
							{#each [['layers', 'Layers'], ['folders', 'Folders']] as [id, label] (id)}
								<button class="rounded-md px-2.5 py-1 {layout === id ? 'bg-background shadow-sm' : 'text-muted-foreground'}" onclick={() => (layout = id as 'layers' | 'folders')}>{label}</button>
							{/each}
						</div>
						<div class="relative">
							<Input list="graph-components" bind:value={focus} placeholder="Focus on a component" class="w-56 pr-8" />
							<datalist id="graph-components">{#each graph.value.components as c (c.id)}<option value={c.id}></option>{/each}</datalist>
							{#if focus}
								<button class="absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground hover:text-foreground" aria-label="Clear" onclick={() => (focus = '')}><X class="size-4" /></button>
							{/if}
						</div>
						{#if focus}
							<NativeSelect bind:value={hops} aria-label="Neighbourhood">
								<option value={1}>direct neighbours</option>
								<option value={2}>two steps away</option>
							</NativeSelect>
						{/if}
						<div class="flex items-center gap-1">
							{#each edgeKinds as k (k.id)}
								<button
									class="rounded-full border px-2.5 py-0.5 text-xs {kinds.has(k.id) ? 'border-foreground/40 bg-foreground/5 text-foreground' : 'text-muted-foreground line-through'}"
									onclick={() => toggleKind(k.id)}
									title="Show {k.label} edges">{k.label}</button
								>
							{/each}
						</div>
						<label class="ml-auto flex items-center gap-2 text-sm text-muted-foreground">
							At most
							<NativeSelect bind:value={limit}>{#each [30, 60, 120, 250, 1000] as n (n)}<option value={n}>{n === 1000 ? 'all' : n}</option>{/each}</NativeSelect>
						</label>
					{:else}
						<span class="text-sm text-muted-foreground">Each folder, then each component, sized by lines of code. Colour:</span>
						<div class="inline-flex rounded-lg bg-muted p-0.5 text-sm">
							{#each [['dependents', 'What depends on it'], ['sensitive', 'Sensitive labels']] as [id, label] (id)}
								<button class="rounded-md px-2.5 py-1 {treeColor === id ? 'bg-background shadow-sm' : 'text-muted-foreground'}" onclick={() => (treeColor = id as 'dependents' | 'sensitive')}>{label}</button>
							{/each}
						</div>
					{/if}
				</div>
				<div class="grid gap-3 p-3 {selected ? 'xl:grid-cols-[minmax(0,1fr)_320px]' : ''}">
					{#if tab === 'graph'}
						<MapCanvas graph={graph.value} {sensitive} {layout} {kinds} focus={focus.trim()} {hops} {limit} bind:selected onopen={(id) => goto(componentHref(id))} />
					{:else}
						<Treemap graph={graph.value} {sensitive} color={treeColor} bind:selected onopen={(id) => goto(componentHref(id))} />
					{/if}
					{#if selected}
						<ComponentPanel
							id={selected}
							onselect={(id) => (selected = id)}
							onfocus={(id) => {
								tab = 'graph';
								focus = id;
							}}
							onclose={() => (selected = '')}
						/>
					{/if}
				</div>
			</Card>
		{:else}
			<Empty title="No components">Onus reads TypeScript and JavaScript workspaces. Declare components in onus.yaml, or add plugins for other languages.</Empty>
		{/if}
	{:else if !graph.error}
		<Skeleton class="h-[60vh] w-full rounded-xl" />
	{/if}
{:else if tab === 'components'}
	<Card pad={false}>
		<div class="border-b px-4 py-3"><Input bind:value={componentText} placeholder="Filter components" class="max-w-xs" /></div>
		<Table.Root>
			<Table.Header>
				<Table.Row>
					<Table.Head class="pl-4">Component</Table.Head>
					<Table.Head>Kind</Table.Head>
					<Table.Head class="text-right">Files</Table.Head>
					<Table.Head class="text-right">Lines</Table.Head>
					<Table.Head class="text-right">Public symbols</Table.Head>
					<Table.Head>Owners</Table.Head>
					<Table.Head class="pr-4">Labels</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each shownComponents as c (c.id)}
					<Table.Row>
						<Table.Cell class="pl-4">
							<a href={componentHref(c.id)} class="font-medium hover:underline">{c.id}</a>
							{#if c.packageName}<div class="font-mono text-xs text-muted-foreground">{c.packageName}</div>{/if}
						</Table.Cell>
						<Table.Cell><Badge tone="faint">{c.kind}</Badge></Table.Cell>
						<Table.Cell class="text-right tabular-nums">{c.files}</Table.Cell>
						<Table.Cell class="text-right tabular-nums">{c.lines.toLocaleString()}</Table.Cell>
						<Table.Cell class="text-right tabular-nums">{c.publicSymbols}</Table.Cell>
						<Table.Cell class="text-xs text-muted-foreground">{c.owners.join(', ')}</Table.Cell>
						<Table.Cell class="pr-4"><div class="flex flex-wrap gap-1">{#each c.labels as l (l)}<Badge tone={sensitive.includes(l) ? 'signal' : 'neutral'}>{l}</Badge>{/each}</div></Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</Card>
{:else if tab === 'files'}
	<Card pad={false}>
		<div class="flex flex-wrap items-center gap-2 border-b px-4 py-3">
			<Input placeholder="Filter by path" bind:value={fileFilter} class="max-w-sm flex-1" />
			<NativeSelect bind:value={componentFilter}>
				<option value="">All components</option>
				{#each graph.value?.components ?? [] as c (c.id)}<option value={c.id}>{c.id}</option>{/each}
			</NativeSelect>
			<NativeSelect bind:value={tests}>
				<option value="all">Code and tests</option>
				<option value="code">Code only</option>
				<option value="tests">Tests only</option>
			</NativeSelect>
			{#if files.value}<span class="ml-auto text-xs text-muted-foreground">{shownFiles.length.toLocaleString()} files{shownFiles.length > 500 ? ', the first 500 shown' : ''}</span>{/if}
		</div>
		{#if files.value}
			<Table.Root>
				<Table.Header>
					<Table.Row>
						<Table.Head class="pl-4">File</Table.Head>
						<Table.Head>Component</Table.Head>
						<Table.Head>Language</Table.Head>
						<Table.Head class="pr-4 text-right">Lines</Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#each shownFiles.slice(0, 500) as f (f.path)}
						<Table.Row>
							<Table.Cell class="pl-4"><a class="font-mono text-xs hover:underline" href={fileHref(f.path)}>{f.path}</a> {#if f.isTest}<Badge tone="faint">test</Badge>{/if}</Table.Cell>
							<Table.Cell>{#if f.component}<a class="hover:underline" href={componentHref(f.component)}>{f.component}</a>{/if}</Table.Cell>
							<Table.Cell class="text-muted-foreground">{f.language}</Table.Cell>
							<Table.Cell class="pr-4 text-right tabular-nums">{f.lines.toLocaleString()}</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
		{:else if files.error}
			<div class="p-4"><ErrorBox error={files.error} /></div>
		{:else}
			<div class="p-4"><Loading /></div>
		{/if}
	</Card>
{:else if tab === 'events'}
	<div class="grid gap-4 xl:grid-cols-2">
		<Card title="Events" subtitle="Who publishes and who consumes, as the extractors in onus.yaml find them." pad={false}>
			{#if graph.value?.events.length}
				<Table.Root>
					<Table.Header><Table.Row><Table.Head class="pl-4">Event</Table.Head><Table.Head>Published by</Table.Head><Table.Head class="pr-4">Consumed by</Table.Head></Table.Row></Table.Header>
					<Table.Body>
						{#each graph.value.events as ev (ev.name)}
							<Table.Row>
								<Table.Cell class="pl-4 font-mono text-xs">{ev.name}</Table.Cell>
								<Table.Cell>{#each ev.publishers as p, i (p)}{i ? ', ' : ''}<a class="hover:underline" href={componentHref(p)}>{p}</a>{/each}</Table.Cell>
								<Table.Cell class="pr-4">{#each ev.consumers as p, i (p)}{i ? ', ' : ''}<a class="hover:underline" href={componentHref(p)}>{p}</a>{/each}</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			{:else}
				<div class="p-4"><Empty title="No events">Declare how events are published and consumed under <code>extractors.events</code>.</Empty></div>
			{/if}
		</Card>
		<Card title="External services" subtitle="Vendors the code calls, and the data that leaves with it." pad={false}>
			{#if graph.value?.externals.length}
				<Table.Root>
					<Table.Header><Table.Row><Table.Head class="pl-4">Service</Table.Head><Table.Head>Category</Table.Head><Table.Head>Data out</Table.Head><Table.Head class="pr-4">Called from</Table.Head></Table.Row></Table.Header>
					<Table.Body>
						{#each graph.value.externals as x (x.id)}
							<Table.Row>
								<Table.Cell class="pl-4">{x.vendor ?? x.id}<div class="font-mono text-xs text-muted-foreground">{x.id}</div></Table.Cell>
								<Table.Cell class="text-muted-foreground">{x.category ?? ''}</Table.Cell>
								<Table.Cell><div class="flex flex-wrap gap-1">{#each x.egress ?? [] as e (e)}<Badge tone="signal">{e}</Badge>{/each}</div></Table.Cell>
								<Table.Cell class="pr-4 text-xs">{graph.value.components.filter((c) => c.externals.includes(x.id)).map((c) => c.id).join(', ')}</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			{:else}
				<div class="p-4"><Empty title="No external services">SDKs Onus knows, and those under <code>extractors.externals</code>, show up here.</Empty></div>
			{/if}
		</Card>
	</div>
{:else}
	<Card pad={false}>
		<form class="flex gap-2 border-b px-4 py-3" onsubmit={find}>
			<div class="relative max-w-xl flex-1">
				<Search class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
				<Input placeholder="Words from a symbol's name, file or docs" bind:value={text} class="pl-8" />
			</div>
			<Button type="submit" disabled={!text.trim()}>Find</Button>
		</form>
		{#if found.running}
			<div class="p-4"><Loading /></div>
		{:else if found.error}
			<div class="p-4"><ErrorBox error={found.error} /></div>
		{:else if found.value}
			<div class="px-4 py-2 text-xs text-muted-foreground">{found.value.total} symbols for “{found.value.query}”</div>
			{#if found.value.symbols.length}
				<Table.Root>
					<Table.Header><Table.Row><Table.Head class="pl-4">Symbol</Table.Head><Table.Head>Kind</Table.Head><Table.Head>Component</Table.Head><Table.Head class="pr-4">Where</Table.Head></Table.Row></Table.Header>
					<Table.Body>
						{#each found.value.symbols as f (f.id)}
							<Table.Row>
								<Table.Cell class="pl-4"><a href={symbolHref(f.id)} class="font-medium hover:underline">{f.name}</a> {#if f.public}<Badge tone="info">public</Badge>{/if}</Table.Cell>
								<Table.Cell class="text-muted-foreground">{f.kind}</Table.Cell>
								<Table.Cell>{#if f.component}<a class="hover:underline" href={idHref(f.component)}>{f.component}</a>{/if}</Table.Cell>
								<Table.Cell class="pr-4">{#if f.file}<a class="font-mono text-xs hover:underline" href={fileHref(f.file, f.line)}>{f.file}:{f.line}</a>{/if}</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			{:else}
				<div class="p-4"><Empty title="No symbols match">{found.value.hint ?? 'Try fewer or other words.'}</Empty></div>
			{/if}
			{#if found.value.files.length}
				<div class="border-t px-4 py-3">
					<p class="mb-2 text-xs font-medium text-muted-foreground">Files</p>
					<ul class="grid gap-1 font-mono text-xs">{#each found.value.files as f (f)}<li><a class="hover:underline" href={fileHref(f)}>{f}</a></li>{/each}</ul>
				</div>
			{/if}
		{:else}
			<div class="p-4"><Empty title="Search the map">Symbols match by name, file, documentation and parameters. ⌘K searches from anywhere.</Empty></div>
		{/if}
	</Card>
{/if}
