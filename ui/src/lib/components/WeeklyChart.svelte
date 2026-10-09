<script lang="ts">
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';

	// Changes per week, stacked by the lane they took, with what went wrong
	// marked under each bar.
	let { weeks }: { weeks: { week: number; counts: Record<string, number> }[] } = $props();

	const lanes = [
		{ id: 'auto-merge', label: 'auto-merge', cls: 'bg-success' },
		{ id: 'judge', label: 'judge', cls: 'bg-info' },
		{ id: 'human', label: 'a person', cls: 'bg-signal' },
		{ id: 'blocked', label: 'blocked', cls: 'bg-destructive' }
	];
	const max = $derived(Math.max(1, ...weeks.map((w) => w.counts.changes ?? 0)));
	const label = (t: number) => new Date(t * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
</script>

<div class="grid gap-3">
	<div class="flex h-48 items-end gap-1.5 border-b pb-px">
		{#each weeks as w (w.week)}
			{@const total = w.counts.changes ?? 0}
			<Tooltip.Root>
				<Tooltip.Trigger class="flex h-full min-w-0 flex-1 flex-col justify-end" aria-label="Week of {label(w.week)}">
					<div class="flex w-full flex-col-reverse overflow-hidden rounded-t-md" style="height: {(total / max) * 100}%">
						{#each lanes as l (l.id)}
							{#if w.counts[l.id]}<div class={l.cls} style="height: {(w.counts[l.id] / total) * 100}%"></div>{/if}
						{/each}
					</div>
				</Tooltip.Trigger>
				<Tooltip.Content>
					<div class="grid gap-0.5 text-xs">
						<span class="font-medium">Week of {label(w.week)}: {total} changes</span>
						{#each lanes as l (l.id)}{#if w.counts[l.id]}<span>{w.counts[l.id]} {l.label}</span>{/if}{/each}
						{#if w.counts.reverted}<span>{w.counts.reverted} reverted</span>{/if}
						{#if w.counts.incidents}<span>{w.counts.incidents} with incidents</span>{/if}
						{#if w.counts.audited}<span>{w.counts.audited} audited, {w.counts.missed ?? 0} missed</span>{/if}
					</div>
				</Tooltip.Content>
			</Tooltip.Root>
		{/each}
	</div>
	<div class="flex gap-1.5">
		{#each weeks as w (w.week)}
			<div class="flex min-w-0 flex-1 flex-col items-center gap-0.5">
				<span class="truncate text-[10px] text-muted-foreground">{label(w.week)}</span>
				{#if w.counts.reverted || w.counts.incidents}
					<span class="size-1.5 rounded-full bg-destructive" title="reverted or incidents"></span>
				{/if}
			</div>
		{/each}
	</div>
	<div class="flex flex-wrap gap-3 text-xs text-muted-foreground">
		{#each lanes as l (l.id)}<span class="flex items-center gap-1.5"><span class="size-2.5 rounded-sm {l.cls}"></span>{l.label}</span>{/each}
		<span class="flex items-center gap-1.5"><span class="size-1.5 rounded-full bg-destructive"></span>reverted or incidents</span>
	</div>
</div>
