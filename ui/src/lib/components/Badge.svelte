<script lang="ts">
	import { cn } from '$lib/utils.js';
	import type { Snippet } from 'svelte';

	export type Tone = 'neutral' | 'signal' | 'add' | 'del' | 'info' | 'ink' | 'faint';

	let {
		tone = 'neutral',
		children,
		title,
		class: className
	}: { tone?: Tone; children: Snippet; title?: string; class?: string } = $props();

	const tones: Record<Tone, string> = {
		neutral: 'bg-muted text-muted-foreground',
		signal: 'bg-signal-soft text-signal-foreground ring-1 ring-inset ring-signal/40',
		add: 'bg-success-soft text-success',
		del: 'bg-danger-soft text-destructive',
		info: 'bg-info-soft text-info',
		ink: 'bg-primary text-primary-foreground',
		faint: 'text-muted-foreground ring-1 ring-inset ring-border'
	};
</script>

<span
	{title}
	class={cn(
		'inline-flex h-5 w-fit shrink-0 items-center gap-1 rounded-full px-2 text-[11px] font-medium whitespace-nowrap [&>svg]:size-3',
		tones[tone],
		className
	)}>{@render children()}</span
>
