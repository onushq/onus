<script lang="ts">
	import type { Snippet } from 'svelte';

	// A labelled control: the label above, an optional hint below.
	let { label, hint, children, class: className = '' }: { label: Snippet | string; hint?: string; children: Snippet; class?: string } =
		$props();
</script>

<label class="grid min-w-0 gap-1.5 text-sm {className}">
	<span class="text-xs font-medium text-muted-foreground">{#if typeof label === 'string'}{label}{:else}{@render label()}{/if}</span>
	{@render children()}
	{#if hint}<span class="text-xs text-muted-foreground">{hint}</span>{/if}
</label>
