<script lang="ts">
	import type { ChangeKind } from '#lib/types.ts';
	import Badge from './Badge.svelte';

	let { kind, label }: { kind: ChangeKind; label?: string } = $props();
	const tone = $derived(
		kind === 'breaking' || kind === 'security-sensitive'
			? 'del'
			: kind === 'additive'
				? 'add'
				: kind === 'dependency' || kind === 'config'
					? 'info'
					: 'neutral'
	);
</script>

<Badge {tone}>{label ?? kind}</Badge>
