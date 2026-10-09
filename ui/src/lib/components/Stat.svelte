<script lang="ts">
	let {
		label,
		value,
		hint,
		href,
		tone
	}: { label: string; value: string | number; hint?: string; href?: string; tone?: 'signal' | 'del' | 'add' } =
		$props();
</script>

<svelte:element this={href ? 'a' : 'div'} class="stat {tone ?? ''}" {href}>
	<span class="label">{label}</span>
	<span class="value">{typeof value === 'number' ? value.toLocaleString() : value}</span>
	{#if hint}<span class="hint">{hint}</span>{/if}
</svelte:element>

<style>
	.stat {
		display: grid;
		gap: 2px;
		padding: var(--space-3) var(--space-4);
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius-m);
		text-decoration: none;
		min-width: 0;
	}
	a.stat:hover {
		border-color: var(--control-border);
	}
	.label {
		font-size: 12px;
		color: var(--ink-muted);
		font-weight: 550;
	}
	.value {
		font-size: 22px;
		font-weight: 650;
		font-variant-numeric: tabular-nums;
		letter-spacing: -0.02em;
	}
	.hint {
		font-size: 12px;
		color: var(--ink-faint);
	}
	.signal {
		border-color: var(--signal);
	}
	.signal .value {
		color: var(--signal-ink);
	}
	.del .value {
		color: var(--del);
	}
	.add .value {
		color: var(--add);
	}
</style>
