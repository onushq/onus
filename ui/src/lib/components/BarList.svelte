<script lang="ts">
	// Labelled horizontal bars, largest first, for small rankings.
	let {
		items,
		format = (n: number) => n.toLocaleString()
	}: { items: { label: string; value: number; href?: string; hint?: string }[]; format?: (n: number) => string } = $props();
	const max = $derived(Math.max(1, ...items.map((i) => i.value)));
</script>

<ul class="grid gap-1.5">
	{#each items as item (item.label)}
		<li class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 text-sm">
			<div class="relative h-7 min-w-0 overflow-hidden rounded-md">
				<div class="absolute inset-y-0 left-0 rounded-md bg-chart-2/15" style="width: {Math.max(2, (item.value / max) * 100)}%"></div>
				<svelte:element this={item.href ? 'a' : 'span'} href={item.href} class="relative flex h-full items-center truncate px-2 hover:underline" title={item.hint}>{item.label}</svelte:element>
			</div>
			<span class="text-right font-mono text-xs text-muted-foreground tabular-nums">{format(item.value)}</span>
		</li>
	{/each}
</ul>
