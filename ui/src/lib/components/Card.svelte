<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle,
		actions,
		children,
		pad = true,
		tone
	}: {
		title?: string;
		subtitle?: string;
		actions?: Snippet;
		children: Snippet;
		pad?: boolean;
		tone?: 'signal';
	} = $props();
</script>

<section class="card" class:signal={tone === 'signal'}>
	{#if title || actions}
		<header>
			<div class="titles">
				{#if title}<h2>{title}</h2>{/if}
				{#if subtitle}<p class="muted small">{subtitle}</p>{/if}
			</div>
			{#if actions}<div class="row">{@render actions()}</div>{/if}
		</header>
	{/if}
	<div class="body" class:pad>{@render children()}</div>
</section>

<style>
	.card {
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius-m);
		min-width: 0;
	}
	.card.signal {
		border-color: var(--signal);
		box-shadow: inset 3px 0 0 var(--signal);
	}
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-3) var(--space-4);
		border-bottom: 1px solid var(--line);
		flex-wrap: wrap;
	}
	.titles {
		display: grid;
		gap: 2px;
		min-width: 0;
	}
	.body {
		overflow-x: auto;
	}
	.pad {
		padding: var(--space-4);
	}
</style>
