<script lang="ts">
	import type { Classification } from '#lib/types.ts';
	import Badge from './Badge.svelte';
	import Inline from './Inline.svelte';
	import LaneBadge from './LaneBadge.svelte';

	let { classification }: { classification: Classification } = $props();
</script>

<div class="grid gap-4">
	<div class="flex items-center gap-3">
		<span class="text-sm text-muted-foreground">Lane</span>
		<LaneBadge lane={classification.lane} large />
	</div>
	<ol class="relative grid gap-3 border-l pl-5">
		{#each classification.applied as a, i (i)}
			<li class="relative text-sm">
				<span class="absolute top-1.5 -left-[25px] size-2.5 rounded-full border-2 border-background bg-muted-foreground/40"></span>
				<div class="flex flex-wrap items-center gap-1.5">
					<Badge tone="faint">{a.source}</Badge>
					<LaneBadge lane={a.lane} />
				</div>
				<p class="mt-1 text-muted-foreground"><Inline text={a.reason} /></p>
			</li>
		{/each}
	</ol>
</div>
