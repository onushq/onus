<script lang="ts">
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import { componentHref, fileHref, symbolHref } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { ComponentInfo, FileRow, Invariants, Tests } from '#lib/types.ts';

	const id = $derived(page.params.id ?? '');
	const info = new Task<ComponentInfo>();
	const files = new Task<{ files: FileRow[] }>();
	const tests = new Task<Tests>();
	const invariants = new Task<Invariants>();

	$effect(() => {
		const c = id;
		info.run(() => query<ComponentInfo>({ query: 'component', id: c }));
		files.run(() => api('map.files', { component: c }));
		tests.run(() => query<Tests>({ query: 'tests-for', target: c, limit: 200 }));
		invariants.run(() => query<Invariants>({ query: 'invariants', target: c }));
	});

	const sorted = (m: Record<string, number>) => Object.entries(m).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
</script>

<PageHead title={id}>
	{#if info.value}
		A {info.value.kind}{info.value.packageName ? `, imported as ${info.value.packageName}` : ''}, in
		<span class="mono">{info.value.roots.join(', ')}</span>
	{/if}
	{#snippet actions()}
		<a class="button" href="/map/symbol?id={encodeURIComponent(id)}&impact=1">What would break?</a>
	{/snippet}
</PageHead>

{#if info.error}
	<ErrorBox error={info.error} />
{:else if !info.value}
	<Loading />
{:else}
	{@const c = info.value}
	<div class="stack">
		<div class="stats">
			<Stat label="Files" value={c.files} />
			<Stat label="Public symbols" value={c.publicSymbols} />
			<Stat label="Uses" value={Object.keys(c.uses).length} hint="components" />
			<Stat label="Used by" value={Object.keys(c.usedBy).length} hint="components" />
			<Stat label="Test files" value={tests.value?.total ?? '…'} />
		</div>

		<div class="grid-2">
			<Card title="Ownership">
				<dl>
					<dt>Owners</dt>
					<dd>{c.owners.length ? c.owners.join(', ') : 'None declared'}</dd>
					<dt>Labels</dt>
					<dd class="row">{#each c.labels as l (l)}<Badge tone="signal">{l}</Badge>{:else}<span class="muted">None</span>{/each}</dd>
					<dt>External services</dt>
					<dd>{c.externals.length ? c.externals.join(', ') : 'None'}</dd>
					<dt>Publishes</dt>
					<dd class="mono small">{c.eventsPublished.join(', ') || '–'}</dd>
					<dt>Consumes</dt>
					<dd class="mono small">{c.eventsConsumed.join(', ') || '–'}</dd>
				</dl>
			</Card>
			<Card title="Relationships" subtitle="Imports, calls and type references between components.">
				<div class="grid-2 tight">
					<div>
						<h3>Uses</h3>
						<ul class="plain">
							{#each sorted(c.uses) as [to, n] (to)}<li><a href={componentHref(to)}>{to}</a> <span class="faint small">{n}</span></li>{:else}<li class="muted">Nothing</li>{/each}
						</ul>
					</div>
					<div>
						<h3>Used by</h3>
						<ul class="plain">
							{#each sorted(c.usedBy) as [from, n] (from)}<li><a href={componentHref(from)}>{from}</a> <span class="faint small">{n}</span></li>{:else}<li class="muted">Nothing</li>{/each}
						</ul>
					</div>
				</div>
			</Card>
		</div>

		{#if invariants.value?.contracts.length}
			<Card title="Declared invariants" subtitle="Rules onus.yaml says a change must keep." tone="signal">
				{#each invariants.value.contracts as k (k.symbol.id)}
					<div class="stack tight">
						<a href={symbolHref(k.symbol.id)}><strong>{k.symbol.name}</strong></a>
						<ul>{#each k.invariants as inv (inv)}<li>{inv}</li>{/each}</ul>
					</div>
				{/each}
			</Card>
		{/if}

		<Card title="Public surface" subtitle="What other components may use." pad={false}>
			{#if c.public.length}
				<table class="data">
					<thead><tr><th>Symbol</th><th>Kind</th><th>Where</th></tr></thead>
					<tbody>
						{#each c.public as s (s.id)}
							<tr>
								<td><a href={symbolHref(s.id)}><strong>{s.name}</strong></a></td>
								<td>{s.kind}</td>
								<td>{#if s.file}<a class="mono small" href={fileHref(s.file, s.line)}>{s.file}:{s.line}</a>{/if}</td>
							</tr>
						{/each}
					</tbody>
				</table>
				{#if c.publicSymbols > c.public.length}<p class="muted small" style="padding: 8px 12px">The first {c.public.length} of {c.publicSymbols}.</p>{/if}
			{:else}
				<div style="padding: 16px"><Empty title="No public symbols" /></div>
			{/if}
		</Card>

		<div class="grid-2">
			<Card title="Files" pad={false}>
				{#if files.value}
					<div class="scroll">
						<table class="data">
							<tbody>
								{#each files.value.files as f (f.path)}
									<tr><td><a class="mono small" href={fileHref(f.path)}>{f.path}</a> {#if f.isTest}<Badge tone="faint">test</Badge>{/if}</td><td class="num small">{f.lines}</td></tr>
								{/each}
							</tbody>
						</table>
					</div>
				{:else}<div style="padding: 16px"><Loading /></div>{/if}
			</Card>
			<Card title="Tests that exercise it" pad={false}>
				{#if tests.value?.tests.length}
					<div class="scroll">
						<table class="data">
							<thead><tr><th>Test file</th><th class="num">Cases</th><th class="num">Skipped</th></tr></thead>
							<tbody>
								{#each tests.value.tests as t (t.file)}
									<tr><td><a class="mono small" href={fileHref(t.file)}>{t.file}</a></td><td class="num">{t.cases}</td><td class="num">{t.skipped || ''}</td></tr>
								{/each}
							</tbody>
						</table>
					</div>
				{:else if tests.value}
					<div style="padding: 16px"><Empty title="No tests reference it" /></div>
				{:else}<div style="padding: 16px"><Loading /></div>{/if}
			</Card>
		</div>
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
	}
	.plain {
		list-style: none;
		padding: 0;
		margin: 8px 0 0;
		display: grid;
		gap: 4px;
	}
	.tight {
		gap: var(--space-3);
	}
	.scroll {
		max-height: 420px;
		overflow: auto;
	}
	ul {
		margin: 0;
	}
</style>
