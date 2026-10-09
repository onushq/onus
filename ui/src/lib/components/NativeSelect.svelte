<script lang="ts">
	import { cn } from '$lib/utils.js';
	import type { Snippet } from 'svelte';
	import type { HTMLSelectAttributes } from 'svelte/elements';

	let {
		value = $bindable(),
		children,
		class: className,
		...rest
	}: HTMLSelectAttributes & { value?: unknown; children: Snippet } = $props();
</script>

<select
	bind:value
	{...rest}
	class={cn(
		'h-8 min-w-0 rounded-lg border border-input bg-transparent px-2.5 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 dark:bg-input/30',
		className
	)}
>
	{@render children()}
</select>
