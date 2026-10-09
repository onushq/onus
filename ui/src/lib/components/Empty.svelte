<script lang="ts">
	import Inbox from '@lucide/svelte/icons/inbox';
	import type { Snippet } from 'svelte';

	let { title, children }: { title: string; children?: Snippet } = $props();
</script>

<div class="flex flex-col items-center gap-2 rounded-lg border border-dashed px-6 py-8 text-center">
	<div class="flex size-9 items-center justify-center rounded-full bg-muted text-muted-foreground">
		<Inbox class="size-4" />
	</div>
	<p class="text-sm font-medium">{title}</p>
	{#if children}<div class="max-w-md text-sm text-muted-foreground">{@render children()}</div>{/if}
</div>
