<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { copy } from '$lib/format.ts';
	import Check from '@lucide/svelte/icons/check';
	import CopyIcon from '@lucide/svelte/icons/copy';

	let { text, label = 'Copy' }: { text: string; label?: string } = $props();
	let done = $state(false);
</script>

<Button
	variant="outline"
	size="sm"
	onclick={async () => {
		done = await copy(text);
		setTimeout(() => (done = false), 1500);
	}}
>
	{#if done}<Check />Copied{:else}<CopyIcon />{label}{/if}
</Button>
