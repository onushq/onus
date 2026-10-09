<script lang="ts">
	import { cn } from '$lib/utils.js';
	import type { Component } from 'svelte';

	let {
		label,
		value,
		hint,
		href,
		tone,
		icon: Icon
	}: {
		label: string;
		value: string | number;
		hint?: string;
		href?: string;
		tone?: 'signal' | 'del' | 'add';
		icon?: Component;
	} = $props();
</script>

<svelte:element
	this={href ? 'a' : 'div'}
	{href}
	class={cn(
		'group relative flex min-w-0 flex-col gap-1 overflow-hidden rounded-xl border bg-gradient-to-t from-muted/40 to-card p-4 no-underline shadow-xs transition-colors',
		href && 'hover:border-ring/60',
		tone === 'signal' && 'border-signal/60 from-signal-soft/50'
	)}
>
	<div class="flex items-center justify-between gap-2 text-xs font-medium text-muted-foreground">
		<span class="truncate">{label}</span>
		{#if Icon}<Icon class="size-4 shrink-0 opacity-70" />{/if}
	</div>
	<div
		class={cn(
			'truncate text-2xl font-semibold tracking-tight tabular-nums',
			tone === 'signal' && 'text-signal-foreground',
			tone === 'del' && 'text-destructive',
			tone === 'add' && 'text-success'
		)}
	>
		{typeof value === 'number' ? value.toLocaleString() : value}
	</div>
	{#if hint}<div class="truncate text-xs text-muted-foreground">{hint}</div>{/if}
</svelte:element>
