<script lang="ts">
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import { api, query } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import BarList from '#lib/components/BarList.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Graph from '#lib/components/Graph.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import { componentHref, fileHref, symbolHref } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { ComponentInfo, FileRow, Graph as G, Invariants, Tests } from '#lib/types.ts';
	import ArrowDownLeft from '@lucide/svelte/icons/arrow-down-left';
	import ArrowUpRight from '@lucide/svelte/icons/arrow-up-right';
	import Braces from '@lucide/svelte/icons/braces';
	import FileCode from '@lucide/svelte/icons/file-code';
	import FlaskConical from '@lucide/svelte/icons/flask-conical';
	import Zap from '@lucide/svelte/icons/zap';

	const id = $derived(page.params.id ?? '');
	const info = new Task<ComponentInfo>();
	const files = new Task<{ files: FileRow[] }>();
	const tests = new Task<Tests>();
	const invariants = new Task<Invariants>();
	const graph = new Task<G>();
	graph.run(() => api<G>('map.graph'));

	$effect(() => {
		const c = id;
		info.run(() => query<ComponentInfo>({ query: 'component', id: c }));
		files.run(() => api('map.files', { component: c }));
		tests.run(() => query<Tests>({ query: 'tests-for', target: c, limit: 200 }));
		invariants.run(() => query<Invariants>({ query: 'invariants', target: c }));
	});

	const sorted = (m: Record<string, number>) =>
		Object.entries(m)
			.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
			.map(([label, value]) => ({ label, value, href: componentHref(label) }));
</script>

<PageHead title={id}>
	{#if info.value}
		A {info.value.kind}{info.value.packageName ? `, imported as ${info.value.packageName}` : ''}, in <code>{info.value.roots.join(', ')}</code>
	{/if}
	{#snippet actions()}
		<Button href="/map/symbol?id={encodeURIComponent(id)}&impact=1"><Zap />What would break?</Button>
	{/snippet}
</PageHead>

{#if info.error}
	<ErrorBox error={info.error} />
{:else if !info.value}
	<div class="grid grid-cols-2 gap-3 md:grid-cols-5">{#each Array(5) as _, i (i)}<Skeleton class="h-24 rounded-xl" />{/each}</div>
{:else}
	{@const c = info.value}
	<div class="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-5">
		<Stat label="Files" value={c.files} icon={FileCode} />
		<Stat label="Public symbols" value={c.publicSymbols} icon={Braces} />
		<Stat label="Uses" value={Object.keys(c.uses).length} icon={ArrowUpRight} hint="components" />
		<Stat label="Used by" value={Object.keys(c.usedBy).length} icon={ArrowDownLeft} hint="components" />
		<Stat label="Test files" value={tests.value?.total ?? '…'} icon={FlaskConical} />
	</div>

	<div class="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
		<Card title="Neighbourhood" subtitle="The components it uses and that use it.">
			{#if graph.value && (Object.keys(c.uses).length || Object.keys(c.usedBy).length)}
				<Graph graph={graph.value} focus={id} limit={30} />
			{:else if graph.value}
				<p class="text-sm text-muted-foreground">Nothing in the map depends on it, and it depends on nothing.</p>
			{:else}
				<Skeleton class="h-64" />
			{/if}
		</Card>
		<div class="grid content-start gap-4">
			<Card title="Ownership">
				<dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
					<dt class="text-muted-foreground">Owners</dt>
					<dd>{c.owners.length ? c.owners.join(', ') : 'None declared'}</dd>
					<dt class="text-muted-foreground">Labels</dt>
					<dd class="flex flex-wrap gap-1">{#each c.labels as l (l)}<Badge tone="signal">{l}</Badge>{:else}<span class="text-muted-foreground">None</span>{/each}</dd>
					<dt class="text-muted-foreground">Externals</dt>
					<dd class="break-words">{c.externals.length ? c.externals.join(', ') : 'None'}</dd>
					<dt class="text-muted-foreground">Publishes</dt>
					<dd class="font-mono text-xs">{c.eventsPublished.join(', ') || '–'}</dd>
					<dt class="text-muted-foreground">Consumes</dt>
					<dd class="font-mono text-xs">{c.eventsConsumed.join(', ') || '–'}</dd>
				</dl>
			</Card>
			<Card title="Uses" subtitle="Imports, calls and type references">
				{#if Object.keys(c.uses).length}<BarList items={sorted(c.uses)} />{:else}<p class="text-sm text-muted-foreground">Nothing</p>{/if}
			</Card>
			<Card title="Used by">
				{#if Object.keys(c.usedBy).length}<BarList items={sorted(c.usedBy)} />{:else}<p class="text-sm text-muted-foreground">Nothing</p>{/if}
			</Card>
		</div>
	</div>

	{#if invariants.value?.contracts.length}
		<Card title="Declared invariants" subtitle="Rules onus.yaml says a change must keep." tone="signal">
			<div class="grid gap-3">
				{#each invariants.value.contracts as k (k.symbol.id)}
					<div>
						<a href={symbolHref(k.symbol.id)} class="font-medium hover:underline">{k.symbol.name}</a>
						<ul class="mt-1 list-disc pl-5 text-sm text-muted-foreground">{#each k.invariants as inv (inv)}<li>{inv}</li>{/each}</ul>
					</div>
				{/each}
			</div>
		</Card>
	{/if}

	<Card title="Public surface" subtitle="What other components may use." pad={false}>
		{#if c.public.length}
			<Table.Root>
				<Table.Header><Table.Row><Table.Head class="pl-4">Symbol</Table.Head><Table.Head>Kind</Table.Head><Table.Head class="pr-4">Where</Table.Head></Table.Row></Table.Header>
				<Table.Body>
					{#each c.public as s (s.id)}
						<Table.Row>
							<Table.Cell class="pl-4"><a href={symbolHref(s.id)} class="font-medium hover:underline">{s.name}</a></Table.Cell>
							<Table.Cell class="text-muted-foreground">{s.kind}</Table.Cell>
							<Table.Cell class="pr-4">{#if s.file}<a class="font-mono text-xs hover:underline" href={fileHref(s.file, s.line)}>{s.file}:{s.line}</a>{/if}</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
			{#if c.publicSymbols > c.public.length}<p class="px-4 py-2 text-xs text-muted-foreground">The first {c.public.length} of {c.publicSymbols}.</p>{/if}
		{:else}
			<div class="p-4"><Empty title="No public symbols" /></div>
		{/if}
	</Card>

	<div class="grid gap-4 xl:grid-cols-2">
		<Card title="Files" subtitle={files.value ? `${files.value.files.length} files` : undefined} pad={false}>
			{#if files.value}
				<div class="max-h-[420px] overflow-auto">
					<Table.Root>
						<Table.Body>
							{#each files.value.files as f (f.path)}
								<Table.Row>
									<Table.Cell class="pl-4"><a class="font-mono text-xs hover:underline" href={fileHref(f.path)}>{f.path}</a> {#if f.isTest}<Badge tone="faint">test</Badge>{/if}</Table.Cell>
									<Table.Cell class="pr-4 text-right text-xs text-muted-foreground tabular-nums">{f.lines}</Table.Cell>
								</Table.Row>
							{/each}
						</Table.Body>
					</Table.Root>
				</div>
			{:else}<div class="p-4"><Loading /></div>{/if}
		</Card>
		<Card title="Tests that exercise it" pad={false}>
			{#if tests.value?.tests.length}
				<div class="max-h-[420px] overflow-auto">
					<Table.Root>
						<Table.Header><Table.Row><Table.Head class="pl-4">Test file</Table.Head><Table.Head class="text-right">Cases</Table.Head><Table.Head class="pr-4 text-right">Skipped</Table.Head></Table.Row></Table.Header>
						<Table.Body>
							{#each tests.value.tests as t (t.file)}
								<Table.Row>
									<Table.Cell class="pl-4"><a class="font-mono text-xs hover:underline" href={fileHref(t.file)}>{t.file}</a></Table.Cell>
									<Table.Cell class="text-right tabular-nums">{t.cases}</Table.Cell>
									<Table.Cell class="pr-4 text-right tabular-nums">{t.skipped || ''}</Table.Cell>
								</Table.Row>
							{/each}
						</Table.Body>
					</Table.Root>
				</div>
			{:else if tests.value}
				<div class="p-4"><Empty title="No tests reference it" /></div>
			{:else}<div class="p-4"><Loading /></div>{/if}
		</Card>
	</div>
{/if}
