<script lang="ts">
	import type { Lane } from '#lib/types.ts';
	import Badge from './Badge.svelte';

	let { lane, large = false }: { lane: Lane; large?: boolean } = $props();
	const tone = $derived(
		lane === 'auto-merge' ? 'add' : lane === 'judge' ? 'info' : lane === 'human' ? 'signal' : 'del'
	);
	const label = $derived(
		lane === 'auto-merge' ? 'auto-merge' : lane === 'judge' ? 'judge' : lane === 'human' ? 'needs a person' : 'blocked'
	);
</script>

<Badge {tone} class={large ? 'h-7 px-3 text-sm' : ''}>{label}</Badge>
