<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Graph from '#lib/components/Graph.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { componentHref, fileHref, idHref, symbolHref } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { FileRow, Found, Graph as G } from '#lib/types.ts';
	import { onMount } from 'svelte';

	const graph = new Task<G>();
	const files = new Task<{ files: FileRow[] }>();
	const found = new Task<Found>();

	let tab = $state(page.url.searchParams.get('tab') ?? (page.url.searchParams.get('q') ? 'find' : 'graph'));
	let text = $state(page.url.searchParams.get('q') ?? '');
	let fileFilter = $state('');
	let componentFilter = $state('');
	let tests = $state<'all' | 'code' | 'tests'>('all');
	let focus = $state(page.url.searchParams.get('focus') ?? '');
	let limit = $state(40);

	onMount(() => {
		graph.run(() => api<G>('map.graph'));
	});

	$effect(() => {
		if (tab === 'files' && !files.value && !files.running) files.run(() => api('map.files'));
	});

	// The header's search box navigates here with ?q=.
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
		goto(`/map?q=${encodeURIComponent(text.trim())}`, { replace: true, reset: false });
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
</script>

<PageHead title="Map" guide="how-it-works">
	The components, contracts and relationships Onus reads from the code and onus.yaml. It follows the
	files on disk as you edit.
</PageHead>

<div class="stack">
	<Tabs
		bind:value={tab}
		tabs={[
			{ id: 'graph', label: 'Graph' },
			{ id: 'components', label: 'Components', count: graph.value?.components.length },
			{ id: 'files', label: 'Files' },
			{ id: 'events', label: 'Events & externals', count: graph.value ? graph.value.events.length + graph.value.externals.length : undefined },
			{ id: 'find', label: 'Find' }
		]}
	/>

	{#if graph.error}<ErrorBox error={graph.error} />{/if}

	{#if tab === 'graph'}
		{#if graph.value}
			{#if graph.value.components.length}
				<div class="row">
					<label class="row small muted">Focus on
						<input list="graph-components" bind:value={focus} placeholder="a component" />
						<datalist id="graph-components">{#each graph.value.components as c (c.id)}<option value={c.id}></option>{/each}</datalist>
					</label>
					{#if focus}<button class="small ghost" onclick={() => (focus = '')}>Clear</button>{/if}
					<label class="row small muted">Show at most
						<select bind:value={limit}>{#each [20, 40, 80, 160, 1000] as n (n)}<option value={n}>{n === 1000 ? 'all' : n}</option>{/each}</select>
					</label>
				</div>
				<Card pad={false}>
					<div style="padding: 12px"><Graph graph={graph.value} {sensitive} {limit} focus={focus.trim()} /></div>
				</Card>
				<p class="muted small">An arrow points from a component to one it uses. Thicker arrows carry more imports, calls and type references; amber outlines mark sensitive labels.</p>
			{:else}
				<Empty title="No components">Onus reads TypeScript and JavaScript workspaces. Declare components in onus.yaml, or add plugins for other languages.</Empty>
			{/if}
		{:else if !graph.error}
			<Loading />
		{/if}
	{:else if tab === 'components'}
		<Card pad={false}>
			<div class="table-wrap">
				<table class="data">
					<thead>
						<tr><th>Component</th><th>Kind</th><th class="num">Files</th><th class="num">Lines</th><th class="num">Public symbols</th><th>Owners</th><th>Labels</th></tr>
					</thead>
					<tbody>
						{#each graph.value?.components ?? [] as c (c.id)}
							<tr>
								<td><a href={componentHref(c.id)}><strong>{c.id}</strong></a>{#if c.packageName}<div class="faint small mono">{c.packageName}</div>{/if}</td>
								<td>{c.kind}</td>
								<td class="num">{c.files}</td>
								<td class="num">{c.lines.toLocaleString()}</td>
								<td class="num">{c.publicSymbols}</td>
								<td class="small">{c.owners.join(', ')}</td>
								<td><div class="row">{#each c.labels as l (l)}<Badge tone={sensitive.includes(l) ? 'signal' : 'neutral'}>{l}</Badge>{/each}</div></td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</Card>
	{:else if tab === 'files'}
		<div class="row">
			<input placeholder="Filter by path" bind:value={fileFilter} style="flex: 1 1 260px" />
			<select bind:value={componentFilter}>
				<option value="">All components</option>
				{#each graph.value?.components ?? [] as c (c.id)}<option value={c.id}>{c.id}</option>{/each}
			</select>
			<select bind:value={tests}>
				<option value="all">Code and tests</option>
				<option value="code">Code only</option>
				<option value="tests">Tests only</option>
			</select>
		</div>
		{#if files.value}
			<Card pad={false}>
				<div class="table-wrap">
					<table class="data">
						<thead><tr><th>File</th><th>Component</th><th>Language</th><th class="num">Lines</th></tr></thead>
						<tbody>
							{#each shownFiles.slice(0, 500) as f (f.path)}
								<tr>
									<td><a class="mono" href={fileHref(f.path)}>{f.path}</a> {#if f.isTest}<Badge tone="faint">test</Badge>{/if}</td>
									<td>{#if f.component}<a href={componentHref(f.component)}>{f.component}</a>{/if}</td>
									<td>{f.language}</td>
									<td class="num">{f.lines.toLocaleString()}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</Card>
			<p class="muted small">{shownFiles.length.toLocaleString()} files{shownFiles.length > 500 ? ', the first 500 shown' : ''}.</p>
		{:else if files.error}
			<ErrorBox error={files.error} />
		{:else}
			<Loading />
		{/if}
	{:else if tab === 'events'}
		<div class="grid-2">
			<Card title="Events" subtitle="Who publishes and who consumes, as the extractors in onus.yaml find them.">
				{#if graph.value?.events.length}
					<table class="data">
						<thead><tr><th>Event</th><th>Published by</th><th>Consumed by</th></tr></thead>
						<tbody>
							{#each graph.value.events as ev (ev.name)}
								<tr>
									<td class="mono">{ev.name}</td>
									<td>{#each ev.publishers as p, i (p)}{i ? ', ' : ''}<a href={componentHref(p)}>{p}</a>{/each}</td>
									<td>{#each ev.consumers as p, i (p)}{i ? ', ' : ''}<a href={componentHref(p)}>{p}</a>{/each}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{:else}
					<Empty title="No events">Declare how events are published and consumed under <code>extractors.events</code>.</Empty>
				{/if}
			</Card>
			<Card title="External services" subtitle="Vendors the code calls, and the data that leaves with it.">
				{#if graph.value?.externals.length}
					<table class="data">
						<thead><tr><th>Service</th><th>Category</th><th>Data out</th><th>Called from</th></tr></thead>
						<tbody>
							{#each graph.value.externals as x (x.id)}
								<tr>
									<td>{x.vendor ?? x.id}<div class="faint small mono">{x.id}</div></td>
									<td>{x.category ?? ''}</td>
									<td>{#each x.egress ?? [] as e (e)}<Badge tone="signal">{e}</Badge> {/each}</td>
									<td class="small">
										{graph.value.components.filter((c) => c.externals.includes(x.id)).map((c) => c.id).join(', ')}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{:else}
					<Empty title="No external services">SDKs Onus knows, and those under <code>extractors.externals</code>, show up here.</Empty>
				{/if}
			</Card>
		</div>
	{:else}
		<form class="row" onsubmit={find}>
			<input placeholder="Words from a symbol's name, file or docs" bind:value={text} style="flex: 1 1 320px" />
			<button class="primary" type="submit" disabled={!text.trim()}>Find</button>
		</form>
		{#if found.running}
			<Loading />
		{:else if found.error}
			<ErrorBox error={found.error} />
		{:else if found.value}
			<Card pad={false} title="{found.value.total} symbols for “{found.value.query}”">
				{#if found.value.symbols.length}
					<table class="data">
						<thead><tr><th>Symbol</th><th>Kind</th><th>Component</th><th>Where</th></tr></thead>
						<tbody>
							{#each found.value.symbols as f (f.id)}
								<tr>
									<td><a href={symbolHref(f.id)}><strong>{f.name}</strong></a> {#if f.public}<Badge tone="info">public</Badge>{/if}</td>
									<td>{f.kind}</td>
									<td>{#if f.component}<a href={idHref(f.component)}>{f.component}</a>{/if}</td>
									<td>{#if f.file}<a class="mono small" href={fileHref(f.file, f.line)}>{f.file}:{f.line}</a>{/if}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{:else}
					<div style="padding: 16px"><Empty title="No symbols match">{found.value.hint ?? 'Try fewer or other words.'}</Empty></div>
				{/if}
			</Card>
			{#if found.value.files.length}
				<Card title="Files">
					<ul class="mono small plain">
						{#each found.value.files as f (f)}<li><a href={fileHref(f)}>{f}</a></li>{/each}
					</ul>
				</Card>
			{/if}
		{/if}
	{/if}
</div>

<style>
	.plain {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 4px;
	}
</style>
