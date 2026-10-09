<script lang="ts">
	import { page } from '$app/state';
	import { query, type ImpactChange } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Shape from '#lib/components/Shape.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { componentHref, fileHref, idHref, symbolHref } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Impact, SymbolInfo, SymbolRef, Tests, Walk } from '#lib/types.ts';

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
	<span class="mono break">{id}</span>
</PageHead>

{#if info.error}
	<ErrorBox error={info.error} />
{:else if info.value?.candidates}
	<Card title="Which one?">
		<ul>
			{#each info.value.candidates as c (c.id)}<li><a href={symbolHref(c.id)}>{c.name}</a> <span class="mono small faint">{c.id}</span></li>{/each}
		</ul>
	</Card>
{:else}
	<div class="stack">
		{#if info.value?.symbol}
			{@const s = info.value.symbol}
			<div class="grid-2">
				<Card title="Symbol">
					<dl>
						<dt>Kind</dt>
						<dd>{s.kind} {#if s.public}<Badge tone="info">public</Badge>{:else}<Badge tone="faint">internal</Badge>{/if}</dd>
						{#if s.component}<dt>Component</dt><dd><a href={componentHref(s.component)}>{s.component}</a></dd>{/if}
						{#if s.file}<dt>Defined in</dt><dd><a class="mono" href={fileHref(s.file, s.line)}>{s.file}:{s.line}</a></dd>{/if}
						<dt>Uses</dt><dd>{info.value.uses}</dd>
						<dt>Used by</dt><dd>{info.value.usedBy}</dd>
						<dt>Tests</dt><dd>{info.value.tests}</dd>
					</dl>
				</Card>
				{#if info.value.invariants.length}
					<Card title="Declared invariants" tone="signal">
						<ul>{#each info.value.invariants as inv (inv)}<li>{inv}</li>{/each}</ul>
					</Card>
				{/if}
				{#if info.value.shape}
					<Card title="Contract shape" subtitle="What a breaking change is measured against.">
						<Shape shape={info.value.shape as never} name={info.value.symbol.name} file={info.value.symbol.file} />
					</Card>
				{/if}
			</div>
		{:else if !isComponent}
			<Loading />
		{/if}

		<Card pad={false}>
			<div style="padding: 8px 16px 0">
				<Tabs
					bind:value={tab}
					tabs={[
						{ id: 'dependents', label: 'What depends on it' },
						{ id: 'dependencies', label: 'What it depends on' },
						{ id: 'tests', label: 'Tests' },
						{ id: 'impact', label: 'Impact of a change' }
					]}
				/>
			</div>
			<div class="pane">
				{#if tab === 'dependents' || tab === 'dependencies'}
					<div class="row">
						<label class="row small muted">Depth
							<select bind:value={depth}>{#each [1, 2, 3, 4, 5] as d (d)}<option value={d}>{d}</option>{/each}</select>
						</label>
						{#if walk.value}<span class="muted small">{walk.value.total} links{walk.value.truncated ? ' (truncated)' : ''} in {Object.keys(walk.value.components).length} components</span>{/if}
					</div>
					{#if walk.running}<Loading />{:else if walk.error}<ErrorBox error={walk.error} />{:else if walk.value}
						{#if walk.value.links.length}
							<table class="data">
								<thead><tr><th>{tab === 'dependents' ? 'Dependent' : 'Dependency'}</th><th>How</th><th>Component</th><th>Where</th><th class="num">Depth</th></tr></thead>
								<tbody>
									{#each walk.value.links as l, i (l.id + i)}
										<tr>
											<td><a href={idHref(l.id)}>{l.name}</a> <span class="faint small">{l.kind}</span></td>
											<td class="small">{l.via}{#if l.confidence !== 'static'} <Badge tone="faint">{l.confidence}</Badge>{/if}</td>
											<td>{#if l.component}<a href={componentHref(l.component)}>{l.component}</a>{/if}</td>
											<td>{#if l.file}<a class="mono small" href={fileHref(l.file, l.line)}>{l.file}{l.line ? `:${l.line}` : ''}</a>{/if}</td>
											<td class="num">{l.depth}</td>
										</tr>
									{/each}
								</tbody>
							</table>
						{:else}
							<Empty title="Nothing found">Static analysis found no {tab === 'dependents' ? 'dependents' : 'dependencies'}. Dynamic dispatch, reflection and other services can hide some; traces add them (<code>--traces</code>).</Empty>
						{/if}
					{/if}
				{:else if tab === 'tests'}
					{#if tests.running}<Loading />{:else if tests.error}<ErrorBox error={tests.error} />{:else if tests.value}
						{#if tests.value.tests.length}
							<table class="data">
								<thead><tr><th>Test file</th><th>Component</th><th class="num">Cases</th><th class="num">Skipped</th></tr></thead>
								<tbody>
									{#each tests.value.tests as t (t.file)}
										<tr><td><a class="mono small" href={fileHref(t.file)}>{t.file}</a></td><td>{t.component ?? ''}</td><td class="num">{t.cases}</td><td class="num">{t.skipped || ''}</td></tr>
									{/each}
								</tbody>
							</table>
						{:else}<Empty title="No tests reference it" />{/if}
					{/if}
				{:else}
					<div class="row">
						<select bind:value={change}>{#each changes as c (c.id)}<option value={c.id}>{c.label}</option>{/each}</select>
						{#if impact.value}<span class="muted small">{impact.value.total} sites{impact.value.truncated ? ' (truncated)' : ''} in {Object.keys(impact.value.components).length} components, {impact.value.tests.length} test files</span>{/if}
					</div>
					{#if impact.running}<Loading />{:else if impact.error}<ErrorBox error={impact.error} />{:else if impact.value}
						{#if impact.value.notes.length}
							<ul class="notes">{#each impact.value.notes as n (n)}<li>{n}</li>{/each}</ul>
						{/if}
						{#if impact.value.sites.length}
							<table class="data">
								<thead><tr><th>Where</th><th>Component</th><th>How it is used</th><th class="num">Depth</th></tr></thead>
								<tbody>
									{#each impact.value.sites as s, i (s.file + i)}
										<tr>
											<td><a class="mono small" href={fileHref(s.file, s.line)}>{s.file}:{s.line}</a> {#if s.test}<Badge tone="faint">test</Badge>{/if}</td>
											<td>{s.component ?? ''}</td>
											<td class="small">{s.uses} <span class="faint">({s.kind})</span></td>
											<td class="num">{s.depth}</td>
										</tr>
									{/each}
								</tbody>
							</table>
						{:else}<Empty title="Nothing would break">No file uses it in a way this change affects.</Empty>{/if}
						{#if impact.value.tests.length}
							<div class="stack tight">
								<h3>Tests to run</h3>
								<ul class="mono small">{#each impact.value.tests as t (t.file)}<li><a href={fileHref(t.file)}>{t.file}</a></li>{/each}</ul>
							</div>
						{/if}
					{/if}
				{/if}
			</div>
		</Card>
	</div>
{/if}

<style>
	dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 6px var(--space-4);
		margin: 0;
	}
	dt {
		color: var(--ink-faint);
	}
	dd {
		margin: 0;
		min-width: 0;
	}
	.pane {
		padding: var(--space-4);
		display: grid;
		gap: var(--space-3);
	}
	.notes {
		margin: 0;
		color: var(--ink-muted);
		font-size: 13px;
	}
	ul {
		margin: 0;
	}
</style>
