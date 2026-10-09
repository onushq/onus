<script lang="ts">
	import * as C from '$lib/components/ui/card/index.js';
	import { cn } from '$lib/utils.js';
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle,
		actions,
		children,
		pad = true,
		tone,
		class: className
	}: {
		title?: string;
		subtitle?: string;
		actions?: Snippet;
		children: Snippet;
		pad?: boolean;
		tone?: 'signal';
		class?: string;
	} = $props();
</script>

<C.Root
	class={cn(
		'min-w-0 gap-0 py-0 shadow-xs',
		tone === 'signal' && 'border-signal/60 ring-1 ring-signal/20',
		className
	)}
>
	{#if title || actions}
		<C.Header class="border-b px-4 py-3 [.border-b]:pb-3">
			{#if title}<C.Title class="text-sm font-semibold">{title}</C.Title>{/if}
			{#if subtitle}<C.Description class="text-xs break-words">{subtitle}</C.Description>{/if}
			{#if actions}<C.Action class="flex items-center gap-2">{@render actions()}</C.Action>{/if}
		</C.Header>
	{/if}
	<C.Content class={cn('overflow-x-auto px-0', pad && 'p-4')}>{@render children()}</C.Content>
</C.Root>
