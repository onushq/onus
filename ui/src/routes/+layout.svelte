<script lang="ts">
	import '@fontsource-variable/archivo';
	import '@fontsource-variable/geist-mono';
	import '../app.css';
	import { page } from '$app/state';
	import * as Breadcrumb from '$lib/components/ui/breadcrumb/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { Toaster } from '$lib/components/ui/sonner/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import AppSidebar from '#lib/components/AppSidebar.svelte';
	import CommandPalette from '#lib/components/CommandPalette.svelte';
	import { app } from '#lib/app.svelte.ts';
	import { ago, short } from '#lib/format.ts';
	import { pageOf } from '#lib/nav/nav.ts';
	import Search from '@lucide/svelte/icons/search';
	import { ModeWatcher } from 'mode-watcher';
	import { onMount } from 'svelte';

	let { children } = $props();
	let palette = $state(false);

	onMount(() => app.start());

	function keys(e: KeyboardEvent) {
		if (e.key === 'k' && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			palette = !palette;
		}
	}

	const current = $derived(pageOf(page.url.pathname));
	// A detail page under a section: its last segment or its id parameter.
	const detail = $derived.by(() => {
		const p = page.url.pathname;
		if (!current || p === current.href) return '';
		const id = page.url.searchParams.get('id') ?? page.url.searchParams.get('path');
		return decodeURIComponent(id ?? p.split('/').filter(Boolean).pop() ?? '');
	});
</script>

<svelte:window onkeydown={keys} />
<ModeWatcher defaultMode="system" />
<Toaster richColors position="bottom-right" />

<Tooltip.Provider delayDuration={200}>
	<Sidebar.Provider style="--sidebar-width: 15rem; --header-height: 3rem;">
		<AppSidebar />
		<Sidebar.Inset class="min-w-0">
			<header class="sticky top-0 z-10 flex h-(--header-height) shrink-0 items-center gap-2 rounded-t-xl border-b bg-background/85 px-3 backdrop-blur lg:px-4">
				<Sidebar.Trigger class="-ml-1" />
				<Separator orientation="vertical" class="mx-1 data-[orientation=vertical]:h-4" />
				<Breadcrumb.Root class="min-w-0">
					<Breadcrumb.List class="flex-nowrap">
						{#if current}
							<Breadcrumb.Item class="shrink-0">
								{#if detail}
									<Breadcrumb.Link href={current.href}>{current.label}</Breadcrumb.Link>
								{:else}
									<Breadcrumb.Page>{current.label}</Breadcrumb.Page>
								{/if}
							</Breadcrumb.Item>
							{#if detail}
								<Breadcrumb.Separator />
								<Breadcrumb.Item class="min-w-0"><Breadcrumb.Page class="truncate">{detail}</Breadcrumb.Page></Breadcrumb.Item>
							{/if}
						{/if}
					</Breadcrumb.List>
				</Breadcrumb.Root>
				<div class="ml-auto flex items-center gap-2">
					{#if app.status}
						<Tooltip.Root>
							<Tooltip.Trigger class="hidden items-center gap-2 rounded-full border px-2.5 py-1 text-xs text-muted-foreground md:flex">
								<span class="relative flex size-2">
									{#if app.status.mapMeta.watching}<span class="absolute inline-flex size-full animate-ping rounded-full bg-success/50"></span>{/if}
									<span class="relative inline-flex size-2 rounded-full {app.status.mapMeta.watching ? 'bg-success' : 'bg-faint'}"></span>
								</span>
								{app.status.map.files.toLocaleString()} files · {app.status.map.symbols.toLocaleString()} symbols
							</Tooltip.Trigger>
							<Tooltip.Content>
								Map v{app.status.mapMeta.version}, built in {app.status.mapMeta.buildMs} ms. HEAD {short(app.status.head.sha)}, {ago(app.status.head.at)}.
								{app.status.mapMeta.watching ? 'It follows the files on disk.' : ''}
							</Tooltip.Content>
						</Tooltip.Root>
					{/if}
					<Button variant="outline" size="sm" class="w-44 justify-start text-muted-foreground sm:w-64" onclick={() => (palette = true)}>
						<Search />
						<span class="truncate">Search…</span>
						<kbd class="ml-auto hidden rounded border bg-muted px-1.5 font-mono text-[10px] sm:inline">⌘K</kbd>
					</Button>
				</div>
			</header>
			<main class="@container/main flex flex-1 flex-col gap-6 p-4 lg:p-6">
				{#if app.error && !app.status}
					<p class="text-sm text-destructive">{app.error}</p>
				{/if}
				{@render children()}
			</main>
		</Sidebar.Inset>
	</Sidebar.Provider>
</Tooltip.Provider>

<CommandPalette bind:open={palette} />
