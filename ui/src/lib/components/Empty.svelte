<script lang="ts">
	import type { Snippet } from 'svelte';

	let { title, children }: { title: string; children?: Snippet } = $props();
</script>

<div class="empty">
	<strong>{title}</strong>
	{#if children}<div class="muted">{@render children()}</div>{/if}
</div>

<style>
	.empty {
		display: grid;
		gap: var(--space-2);
		padding: var(--space-5);
		border: 1px dashed var(--line);
		border-radius: var(--radius-s);
		text-align: center;
		justify-items: center;
	}
	.empty .muted {
		max-width: 520px;
	}
</style>
