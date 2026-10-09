<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import BookOpen from '@lucide/svelte/icons/book-open';
	import type { Snippet } from 'svelte';

	let {
		title,
		children,
		actions,
		guide
	}: { title: string; children?: Snippet; actions?: Snippet; guide?: string } = $props();
</script>

<svelte:head><title>{title} · Onus</title></svelte:head>

<div class="flex flex-wrap items-start justify-between gap-4">
	<div class="grid max-w-3xl gap-1">
		<h1 class="text-xl font-semibold tracking-tight">{title}</h1>
		{#if children}<p class="text-sm text-muted-foreground">{@render children()}</p>{/if}
	</div>
	<div class="flex items-center gap-2">
		{#if actions}{@render actions()}{/if}
		{#if guide}<Button variant="ghost" size="sm" href="/guide/{guide}"><BookOpen />Guide</Button>{/if}
	</div>
</div>
