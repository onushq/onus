<script lang="ts">
	import { goto } from '$app/navigation';
	import * as Command from '$lib/components/ui/command/index.js';
	import { api, query } from '#lib/api.ts';
	import { componentHref, fileHref, symbolHref } from '#lib/format.ts';
	import { pages } from '#lib/nav/nav.ts';
	import type { Found, Graph } from '#lib/types.ts';
	import Box from '@lucide/svelte/icons/box';
	import Braces from '@lucide/svelte/icons/braces';
	import FileCode from '@lucide/svelte/icons/file-code';
	import Search from '@lucide/svelte/icons/search';

	let { open = $bindable(false) }: { open: boolean } = $props();
	let text = $state('');
	let found = $state<Found | undefined>();
	let components = $state<string[]>([]);
	let timer: ReturnType<typeof setTimeout> | undefined;

	$effect(() => {
		if (open && !components.length) {
			api<Graph>('map.graph').then((g) => (components = g.components.map((c) => c.id).sort()));
		}
	});

	// The map search runs as you type, a moment after you stop.
	$effect(() => {
		const t = text.trim();
		clearTimeout(timer);
		if (t.length < 2) {
			found = undefined;
			return;
		}
		timer = setTimeout(async () => {
			try {
				found = await query<Found>({ query: 'find', text: t, limit: 12 });
			} catch {
				found = undefined;
			}
		}, 160);
	});

	function go(href: string) {
		open = false;
		text = '';
		goto(href);
	}

	const shownComponents = $derived(
		text.trim() ? components.filter((c) => c.toLowerCase().includes(text.trim().toLowerCase())).slice(0, 8) : []
	);
</script>

<Command.Dialog bind:open shouldFilter={false} title="Search Onus" description="Pages, components, symbols and files">
	<Command.Input placeholder="Search pages, components, symbols and files…" bind:value={text} />
	<Command.List>
		<Command.Empty>Nothing found. Try other words.</Command.Empty>
		{#if found?.symbols.length}
			<Command.Group heading="Symbols">
				{#each found.symbols as s (s.id)}
					<Command.Item value={s.id} onSelect={() => go(symbolHref(s.id))}>
						<Braces />
						<span class="font-medium">{s.name}</span>
						<span class="truncate text-xs text-muted-foreground">{s.kind}{s.component ? ` · ${s.component}` : ''}</span>
					</Command.Item>
				{/each}
			</Command.Group>
		{/if}
		{#if shownComponents.length}
			<Command.Group heading="Components">
				{#each shownComponents as c (c)}
					<Command.Item value={'component:' + c} onSelect={() => go(componentHref(c))}><Box />{c}</Command.Item>
				{/each}
			</Command.Group>
		{/if}
		{#if found?.files.length}
			<Command.Group heading="Files">
				{#each found.files.slice(0, 8) as f (f)}
					<Command.Item value={'file:' + f} onSelect={() => go(fileHref(f))}><FileCode /><span class="truncate font-mono text-xs">{f}</span></Command.Item>
				{/each}
			</Command.Group>
		{/if}
		{#if text.trim().length >= 2}
			<Command.Group heading="Search">
				<Command.Item value="search-all" onSelect={() => go(`/map?q=${encodeURIComponent(text.trim())}`)}>
					<Search />All results for “{text.trim()}”
				</Command.Item>
			</Command.Group>
		{/if}
		<Command.Group heading="Pages">
			{#each pages.filter((p) => !text.trim() || p.label.toLowerCase().includes(text.trim().toLowerCase())) as p (p.href)}
				<Command.Item value={'page:' + p.href} onSelect={() => go(p.href)}>
					<p.icon />{p.label}<span class="ml-auto truncate text-xs text-muted-foreground">{p.hint}</span>
				</Command.Item>
			{/each}
		</Command.Group>
	</Command.List>
</Command.Dialog>
