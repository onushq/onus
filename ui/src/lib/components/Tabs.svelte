<script lang="ts">
	import { cn } from '$lib/utils.js';

	let {
		tabs,
		value = $bindable()
	}: { tabs: { id: string; label: string; count?: number }[]; value: string } = $props();
</script>

<div
	role="tablist"
	class="inline-flex h-9 w-fit max-w-full items-center gap-1 overflow-x-auto rounded-lg bg-muted p-1 text-muted-foreground"
>
	{#each tabs as t (t.id)}
		<button
			role="tab"
			aria-selected={value === t.id}
			onclick={() => (value = t.id)}
			class={cn(
				'inline-flex h-7 items-center gap-1.5 rounded-md px-3 text-sm font-medium whitespace-nowrap transition-all hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none',
				value === t.id && 'bg-background text-foreground shadow-sm dark:bg-input/40'
			)}
		>
			{t.label}
			{#if t.count !== undefined}
				<span class="rounded-full bg-muted-foreground/15 px-1.5 text-[11px] tabular-nums">{t.count}</span>
			{/if}
		</button>
	{/each}
</div>
