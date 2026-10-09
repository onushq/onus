<script lang="ts">
	let {
		tabs,
		value = $bindable()
	}: { tabs: { id: string; label: string; count?: number }[]; value: string } = $props();
</script>

<div class="tabs" role="tablist">
	{#each tabs as t (t.id)}
		<button
			role="tab"
			class="ghost"
			class:active={value === t.id}
			aria-selected={value === t.id}
			onclick={() => (value = t.id)}
		>
			{t.label}
			{#if t.count !== undefined}<span class="count">{t.count}</span>{/if}
		</button>
	{/each}
</div>

<style>
	.tabs {
		display: flex;
		gap: var(--space-1);
		border-bottom: 1px solid var(--line);
		overflow-x: auto;
	}
	button {
		border-radius: var(--radius-s) var(--radius-s) 0 0;
		color: var(--ink-muted);
		border-bottom: 2px solid transparent;
		margin-bottom: -1px;
	}
	button.active {
		color: var(--ink);
		border-bottom-color: var(--ink);
	}
	.count {
		font-size: 11px;
		background: var(--sunken);
		border-radius: var(--radius-pill);
		padding: 0 6px;
		color: var(--ink-muted);
	}
</style>
