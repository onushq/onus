<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		children,
		actions,
		guide
	}: { title: string; children?: Snippet; actions?: Snippet; guide?: string } = $props();
</script>

<svelte:head><title>{title} · Onus</title></svelte:head>

<div class="head">
	<div class="text">
		<h1>{title}</h1>
		{#if children}<p class="muted">{@render children()}</p>{/if}
	</div>
	<div class="row">
		{#if actions}{@render actions()}{/if}
		{#if guide}<a class="button ghost" href="/guide/{guide}">Guide</a>{/if}
	</div>
</div>

<style>
	.head {
		display: flex;
		justify-content: space-between;
		align-items: flex-start;
		gap: var(--space-4);
		margin-bottom: var(--space-5);
		flex-wrap: wrap;
	}
	.text {
		display: grid;
		gap: var(--space-1);
		max-width: 760px;
	}
</style>
