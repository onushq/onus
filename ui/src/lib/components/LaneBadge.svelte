<script lang="ts">
	import type { Lane } from '#lib/types.ts';
	import Badge from './Badge.svelte';

	let { lane, large = false }: { lane: Lane; large?: boolean } = $props();
	const tone = $derived(
		lane === 'auto-merge' ? 'add' : lane === 'judge' ? 'info' : lane === 'human' ? 'signal' : 'del'
	);
	const label = $derived(
		lane === 'auto-merge'
			? 'auto-merge'
			: lane === 'judge'
				? 'judge'
				: lane === 'human'
					? 'needs a person'
					: 'blocked'
	);
</script>

<span class:large><Badge {tone}>{label}</Badge></span>

<style>
	.large :global(.badge) {
		height: 26px;
		padding: 0 12px;
		font-size: 13px;
	}
</style>
