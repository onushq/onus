<script lang="ts">
	import { page } from '$app/state';
	import * as Table from '$lib/components/ui/table/index.js';
	import { query, type ImpactChange } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import NativeSelect from '#lib/components/NativeSelect.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Shape from '#lib/components/Shape.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { componentHref, fileHref, idHref, symbolHref } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Impact, SymbolInfo, SymbolRef, Tests, Walk } from '#lib/types.ts';
	import ArrowDownLeft from '@lucide/svelte/icons/arrow-down-left';
	import ArrowUpRight from '@lucide/svelte/icons/arrow-up-right';
	import FlaskConical from '@lucide/svelte/icons/flask-conical';

	const id = $derived(page.url.searchParams.get('id') ?? '');
	const info = new Task<SymbolInfo & { candidates?: SymbolRef[] }>();
	const walk = new Task<Walk>();
	const tests = new Task<Tests>();
	const impact = new Task<Impact>();

	let tab = $state(page.url.searchParams.get('impact') ? 'impact' : 'dependents');
	let depth = $state(1);
	let change = $state<ImpactChange>('change-signature');

	const isComponent = $derived(!id.includes('#') && !id.includes('/'));

	$effect(() => {
		const target = id;
		if (!isComponent) info.run(() => query({ query: 'symbol', id: target }));
	});

	$effect(() => {
		const target = id;
		if (tab === 'dependents' || tab === 'dependencies') {
			const q = tab;
			const d = depth;
			walk.run(() => query<Walk>({ query: q, target, depth: d, limit: 300 }));
		} else if (tab === 'tests') {
			tests.run(() => query<Tests>({ query: 'tests-for', target, limit: 300 }));
		} else if (tab === 'impact') {
			const c = change;
			impact.run(() => query<Impact>({ query: 'impact', target, change: c, limit: 300 }));
		}
	});

	const changes: { id: ImpactChange; label: string }[] = [
		{ id: 'change-signature', label: 'Change its signature' },
		{ id: 'remove', label: 'Remove it' },
		{ id: 'rename', label: 'Rename it' },
		{ id: 'add-required-member', label: 'Add a required member' },
		{ id: 'change-behavior', label: 'Change only its behavior' }
	];
</script>

<PageHead title={info.value?.symbol?.name ?? id}>
	<code class="break-all">{id}</code>
</PageHead>

{#if info.error}
	<ErrorBox error={info.error} />
{:else if info.value?.candidates}
	<Card title="Which one?">
		<ul class="grid gap-1 text-sm">
			{#each info.value.candidates as c (c.id)}<li><a class="font-medium hover:underline" href={symbolHref(c.id)}>{c.name}</a> <span class="font-mono text-xs text-muted-foreground">{c.id}</span></li>{/each}
		</ul>
	</Card>
{:else}
	{#if info.value?.symbol}
		{@const s = info.value.symbol}
		<div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
			<div class="grid content-start gap-4">
				<div class="grid grid-cols-3 gap-3">
					<Stat label="Uses" value={info.value.uses} icon={ArrowUpRight} />
					<Stat label="Used by" value={info.value.usedBy} icon={ArrowDownLeft} />
					<Stat label="Tests" value={info.value.tests} icon={FlaskConical} />
				</div>
				<Card title="Symbol">
					<dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
						<dt class="text-muted-foreground">Kind</dt>
						<dd class="flex items-center gap-2">{s.kind} {#if s.public}<Badge tone="info">public</Badge>{:else}<Badge tone="faint">internal</Badge>{/if}</dd>
						{#if s.component}<dt class="text-muted-foreground">Component</dt><dd><a class="hover:underline" href={componentHref(s.component)}>{s.component}</a></dd>{/if}
						{#if s.file}<dt class="text-muted-foreground">Defined in</dt><dd><a class="font-mono text-xs break-all hover:underline" href={fileHref(s.file, s.line)}>{s.file}:{s.line}</a></dd>{/if}
					</dl>
				</Card>
				{#if info.value.invariants.length}
					<Card title="Declared invariants" tone="signal">
						<ul class="list-disc pl-5 text-sm">{#each info.value.invariants as inv (inv)}<li>{inv}</li>{/each}</ul>
					</Card>
				{/if}
			</div>
			{#if info.value.shape}
				<Card title="Contract shape" subtitle="What a breaking change is measured against." class="self-start">
					<Shape shape={info.value.shape as never} name={s.name} file={s.file} />
				</Card>
			{/if}
		</div>
	{:else if !isComponent}
		<Loading />
	{/if}

	<Card pad={false}>
		<div class="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-3">
			<Tabs
				bind:value={tab}
				tabs={[
					{ id: 'dependents', label: 'Depends on it' },
					{ id: 'dependencies', label: 'It depends on' },
					{ id: 'tests', label: 'Tests' },
					{ id: 'impact', label: 'Impact of a change' }
				]}
			/>
			{#if tab === 'dependents' || tab === 'dependencies'}
				<label class="flex items-center gap-2 text-sm text-muted-foreground">Depth
					<NativeSelect bind:value={depth}>{#each [1, 2, 3, 4, 5] as d (d)}<option value={d}>{d}</option>{/each}</NativeSelect>
				</label>
			{:else if tab === 'impact'}
				<NativeSelect bind:value={change}>{#each changes as c (c.id)}<option value={c.id}>{c.label}</option>{/each}</NativeSelect>
			{/if}
		</div>
		<div>
			{#if tab === 'dependents' || tab === 'dependencies'}
				{#if walk.running}<div class="p-4"><Loading /></div>{:else if walk.error}<div class="p-4"><ErrorBox error={walk.error} /></div>{:else if walk.value}
					<p class="px-4 py-2 text-xs text-muted-foreground">{walk.value.total} links{walk.value.truncated ? ' (truncated)' : ''} in {Object.keys(walk.value.components).length} components</p>
					{#if walk.value.links.length}
						<Table.Root>
							<Table.Header><Table.Row><Table.Head class="pl-4">{tab === 'dependents' ? 'Dependent' : 'Dependency'}</Table.Head><Table.Head>How</Table.Head><Table.Head>Component</Table.Head><Table.Head>Where</Table.Head><Table.Head class="pr-4 text-right">Depth</Table.Head></Table.Row></Table.Header>
							<Table.Body>
								{#each walk.value.links as l, i (l.id + i)}
									<Table.Row>
										<Table.Cell class="pl-4"><a class="font-medium hover:underline" href={idHref(l.id)}>{l.name}</a> <span class="text-xs text-muted-foreground">{l.kind}</span></Table.Cell>
										<Table.Cell class="text-xs">{l.via}{#if l.confidence !== 'static'} <Badge tone="faint">{l.confidence}</Badge>{/if}</Table.Cell>
										<Table.Cell>{#if l.component}<a class="hover:underline" href={componentHref(l.component)}>{l.component}</a>{/if}</Table.Cell>
										<Table.Cell>{#if l.file}<a class="font-mono text-xs hover:underline" href={fileHref(l.file, l.line)}>{l.file}{l.line ? `:${l.line}` : ''}</a>{/if}</Table.Cell>
										<Table.Cell class="pr-4 text-right tabular-nums">{l.depth}</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					{:else}
						<div class="p-4"><Empty title="Nothing found">Static analysis found none. Dynamic dispatch, reflection and other services can hide some; traces add them (<code>--traces</code>).</Empty></div>
					{/if}
				{/if}
			{:else if tab === 'tests'}
				{#if tests.running}<div class="p-4"><Loading /></div>{:else if tests.error}<div class="p-4"><ErrorBox error={tests.error} /></div>{:else if tests.value}
					{#if tests.value.tests.length}
						<Table.Root>
							<Table.Header><Table.Row><Table.Head class="pl-4">Test file</Table.Head><Table.Head>Component</Table.Head><Table.Head class="text-right">Cases</Table.Head><Table.Head class="pr-4 text-right">Skipped</Table.Head></Table.Row></Table.Header>
							<Table.Body>
								{#each tests.value.tests as t (t.file)}
									<Table.Row>
										<Table.Cell class="pl-4"><a class="font-mono text-xs hover:underline" href={fileHref(t.file)}>{t.file}</a></Table.Cell>
										<Table.Cell>{t.component ?? ''}</Table.Cell>
										<Table.Cell class="text-right tabular-nums">{t.cases}</Table.Cell>
										<Table.Cell class="pr-4 text-right tabular-nums">{t.skipped || ''}</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					{:else}<div class="p-4"><Empty title="No tests reference it" /></div>{/if}
				{/if}
			{:else}
				{#if impact.running}<div class="p-4"><Loading /></div>{:else if impact.error}<div class="p-4"><ErrorBox error={impact.error} /></div>{:else if impact.value}
					<p class="px-4 py-2 text-xs text-muted-foreground">{impact.value.total} sites{impact.value.truncated ? ' (truncated)' : ''} in {Object.keys(impact.value.components).length} components · {impact.value.tests.length} test files to run</p>
					{#if impact.value.notes.length}
						<ul class="mx-4 mb-3 grid gap-1 rounded-lg bg-muted/60 px-4 py-2 text-xs text-muted-foreground">{#each impact.value.notes as n (n)}<li>{n}</li>{/each}</ul>
					{/if}
					{#if impact.value.sites.length}
						<Table.Root>
							<Table.Header><Table.Row><Table.Head class="pl-4">Where</Table.Head><Table.Head>Component</Table.Head><Table.Head>How it is used</Table.Head><Table.Head class="pr-4 text-right">Depth</Table.Head></Table.Row></Table.Header>
							<Table.Body>
								{#each impact.value.sites as s, i (s.file + i)}
									<Table.Row>
										<Table.Cell class="pl-4"><a class="font-mono text-xs hover:underline" href={fileHref(s.file, s.line)}>{s.file}:{s.line}</a> {#if s.test}<Badge tone="faint">test</Badge>{/if}</Table.Cell>
										<Table.Cell>{s.component ?? ''}</Table.Cell>
										<Table.Cell class="text-xs">{s.uses} <span class="text-muted-foreground">({s.kind})</span></Table.Cell>
										<Table.Cell class="pr-4 text-right tabular-nums">{s.depth}</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					{:else}<div class="p-4"><Empty title="Nothing would break">No file uses it in a way this change affects.</Empty></div>{/if}
					{#if impact.value.tests.length}
						<div class="border-t px-4 py-3">
							<p class="mb-2 text-xs font-medium text-muted-foreground">Tests to run</p>
							<ul class="grid gap-1 font-mono text-xs">{#each impact.value.tests as t (t.file)}<li><a class="hover:underline" href={fileHref(t.file)}>{t.file}</a></li>{/each}</ul>
						</div>
					{/if}
				{/if}
			{/if}
		</div>
	</Card>
{/if}
