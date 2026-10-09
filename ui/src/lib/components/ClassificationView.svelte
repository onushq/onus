<script lang="ts">
	import type { Classification } from '#lib/types.ts';
	import Badge from './Badge.svelte';
	import Inline from './Inline.svelte';
	import LaneBadge from './LaneBadge.svelte';

	let { classification }: { classification: Classification } = $props();
</script>

<div class="stack tight">
	<div class="row"><span class="muted">Lane</span> <LaneBadge lane={classification.lane} large /></div>
	<ol class="applied">
		{#each classification.applied as a, i (i)}
			<li>
				<Badge tone="faint">{a.source}</Badge>
				<LaneBadge lane={a.lane} />
				<span><Inline text={a.reason} /></span>
			</li>
		{/each}
	</ol>
</div>

<style>
	.applied {
		margin: 0;
		padding-left: 1.2em;
		display: grid;
		gap: var(--space-2);
		font-size: 13px;
	}
	.applied li > :global(*) {
		margin-right: 6px;
	}
</style>
