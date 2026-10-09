<script lang="ts">
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import CodeView from '#lib/components/CodeView.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { componentHref, fileHref, symbolHref } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { FileInfo } from '#lib/types.ts';

	const path = $derived(page.url.searchParams.get('path') ?? '');
	const line = $derived(Number(page.url.searchParams.get('line') ?? 0));
	const end = $derived(Number(page.url.searchParams.get('end') ?? 0));
	const info = new Task<FileInfo>();
	const text = new Task<{ text: string }>();

	$effect(() => {
		const p = path;
		info.run(() => query<FileInfo>({ query: 'file', path: p }));
		text.run(() => api('files.read', { path: p }));
	});
</script>

<PageHead title={path.split('/').pop() ?? path}>
	<span class="mono break">{path}</span>
	{#if info.value?.component}in <a href={componentHref(info.value.component)}>{info.value.component}</a>{/if}
</PageHead>

<div class="layout">
	<div class="code">
		{#if text.error}
			<ErrorBox error={text.error} title="Cannot show the file" />
		{:else if text.value}
			<CodeView text={text.value.text} from={line} to={end} />
		{:else}
			<Loading />
		{/if}
	</div>
	<div class="stack side">
		{#if info.value}
			<Card title="Symbols">
				<ul class="plain">
					{#each info.value.symbols as s (s.id)}
						<li>
							<a href={symbolHref(s.id)}>{s.name}</a>
							<span class="faint small">{s.kind}</span>
							{#if s.public}<Badge tone="info">public</Badge>{/if}
							{#if s.line}<a class="faint small" href={fileHref(path, s.line)}>:{s.line}</a>{/if}
						</li>
					{:else}<li class="muted">None</li>{/each}
				</ul>
			</Card>
			<Card title="Imports">
				<ul class="plain mono small">
					{#each info.value.imports as i (i)}
						<li>{#if i.startsWith('npm:') || i.startsWith('?')}{i}{:else}<a href={fileHref(i)}>{i}</a>{/if}</li>
					{:else}<li class="muted">None</li>{/each}
				</ul>
			</Card>
			<Card title="Imported by">
				<ul class="plain mono small">
					{#each info.value.importedBy as i (i)}<li><a href={fileHref(i)}>{i}</a></li>{:else}<li class="muted">Nothing</li>{/each}
				</ul>
			</Card>
			{#if info.value.diagnostics.length}
				<Card title="Map notes"><ul>{#each info.value.diagnostics as d (d)}<li class="small">{d}</li>{/each}</ul></Card>
			{/if}
		{:else if info.error}
			<p class="muted small">Not in the map: {info.error}</p>
		{/if}
	</div>
</div>

<style>
	.layout {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 300px;
		gap: var(--space-4);
		align-items: start;
	}
	@media (max-width: 1000px) {
		.layout {
			grid-template-columns: 1fr;
		}
	}
	.plain {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 4px;
		overflow-wrap: anywhere;
	}
</style>
