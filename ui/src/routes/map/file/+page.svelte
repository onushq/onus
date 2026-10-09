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
	<code class="break-all">{path}</code>
	{#if info.value?.component}in <a class="underline" href={componentHref(info.value.component)}>{info.value.component}</a>{/if}
</PageHead>

<div class="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_300px]">
	<div class="min-w-0">
		{#if text.error}
			<ErrorBox error={text.error} title="Cannot show the file" />
		{:else if text.value}
			<CodeView text={text.value.text} from={line} to={end} />
		{:else}
			<Loading />
		{/if}
	</div>
	<div class="grid gap-4">
		{#if info.value}
			<Card title="Symbols" subtitle="{info.value.symbols.length} declared">
				<ul class="grid gap-1.5 text-sm">
					{#each info.value.symbols as s (s.id)}
						<li class="flex items-center gap-2">
							<a class="min-w-0 truncate font-medium hover:underline" href={symbolHref(s.id)}>{s.name}</a>
							<span class="text-xs text-muted-foreground">{s.kind}</span>
							{#if s.public}<Badge tone="info">public</Badge>{/if}
							{#if s.line}<a class="ml-auto font-mono text-xs text-muted-foreground hover:underline" href={fileHref(path, s.line)}>:{s.line}</a>{/if}
						</li>
					{:else}<li class="text-muted-foreground">None</li>{/each}
				</ul>
			</Card>
			<Card title="Imports">
				<ul class="grid gap-1 font-mono text-xs break-all">
					{#each info.value.imports as i (i)}
						<li>{#if i.startsWith('npm:') || i.startsWith('?')}<span class="text-muted-foreground">{i}</span>{:else}<a class="hover:underline" href={fileHref(i)}>{i}</a>{/if}</li>
					{:else}<li class="text-muted-foreground">None</li>{/each}
				</ul>
			</Card>
			<Card title="Imported by">
				<ul class="grid gap-1 font-mono text-xs break-all">
					{#each info.value.importedBy as i (i)}<li><a class="hover:underline" href={fileHref(i)}>{i}</a></li>{:else}<li class="text-muted-foreground">Nothing</li>{/each}
				</ul>
			</Card>
			{#if info.value.diagnostics.length}
				<Card title="Map notes"><ul class="grid gap-1 text-xs">{#each info.value.diagnostics as d (d)}<li>{d}</li>{/each}</ul></Card>
			{/if}
		{:else if info.error}
			<p class="text-sm text-muted-foreground">Not in the map: {info.error}</p>
		{/if}
	</div>
</div>
