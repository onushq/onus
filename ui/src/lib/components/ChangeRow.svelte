<script lang="ts">
	import { fileHref, idHref } from '#lib/format.ts';
	import type { SemanticChange } from '#lib/types.ts';
	import { cn } from '$lib/utils.js';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import Badge from './Badge.svelte';
	import Inline from './Inline.svelte';
	import KindBadge from './KindBadge.svelte';

	let { change, open = false }: { change: SemanticChange; open?: boolean } = $props();
	let expanded = $state(false);
	$effect(() => {
		expanded = open;
	});
	const person = $derived(change.hints.needsPerson);
</script>

<article class={cn('border-b last:border-b-0', person && 'bg-signal-soft/25 shadow-[inset_3px_0_0_var(--signal)]')}>
	<button
		class="flex w-full items-start gap-3 px-4 py-3 text-left hover:bg-muted/40 focus-visible:bg-muted/40 focus-visible:outline-none"
		onclick={() => (expanded = !expanded)}
		aria-expanded={expanded}
	>
		<ChevronRight class={cn('mt-0.5 size-4 shrink-0 text-muted-foreground transition-transform', expanded && 'rotate-90')} />
		<div class="grid min-w-0 flex-1 gap-1">
			<div class="flex flex-wrap items-center gap-x-2 gap-y-1">
				<span class="font-medium break-words"><Inline text={change.title} /></span>
			</div>
			<p class="text-sm text-muted-foreground break-words"><Inline text={change.whyItMatters} /></p>
		</div>
		<div class="flex shrink-0 flex-wrap justify-end gap-1">
			{#if person}<Badge tone="signal">needs a person</Badge>{/if}
			{#if change.hints.intentMismatch}<Badge tone="signal">outside intent</Badge>{/if}
			{#if change.hints.rulesOfTheGame}<Badge tone="signal">rules of the game</Badge>{/if}
			<KindBadge kind={change.kind} label={change.kindLabel} />
		</div>
	</button>
	{#if expanded}
		<div class="grid gap-4 px-11 pb-4 text-sm md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
			<dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1.5">
				<dt class="text-muted-foreground">Subject</dt>
				<dd><a href={idHref(change.subject)} class="font-mono text-xs break-all hover:underline">{change.subject}</a></dd>
				<dt class="text-muted-foreground">Subkind</dt>
				<dd class="font-mono text-xs">{change.subkind}</dd>
				{#if change.component}
					<dt class="text-muted-foreground">Component</dt>
					<dd><a href={idHref(change.component)} class="hover:underline">{change.component}</a></dd>
				{/if}
				<dt class="text-muted-foreground">Blast radius</dt>
				<dd>{change.hints.blastRadius} dependent files</dd>
				<dt class="text-muted-foreground">Confidence</dt>
				<dd>{change.hints.confidence}</dd>
				{#if change.hints.labels.length}
					<dt class="text-muted-foreground">Labels</dt>
					<dd class="flex flex-wrap gap-1">{#each change.hints.labels as l (l)}<Badge tone="signal">{l}</Badge>{/each}</dd>
				{/if}
				{#if change.hints.novelty.length}
					<dt class="text-muted-foreground">New</dt>
					<dd>{change.hints.novelty.join(', ')}</dd>
				{/if}
				{#if change.stats}
					<dt class="text-muted-foreground">Size</dt>
					<dd>
						<span class="text-success">+{change.stats.linesAdded}</span>
						<span class="text-destructive">−{change.stats.linesRemoved}</span>
						in {change.stats.files} files
					</dd>
				{/if}
				<dt class="text-muted-foreground">Row id</dt>
				<dd class="font-mono text-xs break-all text-muted-foreground">{change.id}</dd>
			</dl>
			{#if change.locations.length}
				<ul class="grid content-start gap-1.5">
					{#each change.locations as l, i (i)}
						{@const where = `${l.file}:${l.lines[0]}${l.lines[1] !== l.lines[0] ? `–${l.lines[1]}` : ''}`}
						<li class="flex items-center gap-2 text-xs">
							<Badge tone="faint">{l.side}</Badge>
							{#if l.side === 'head'}
								<a class="font-mono break-all hover:underline" href={fileHref(l.file, l.lines[0], l.lines[1])}>{where}</a>
							{:else}
								<span class="font-mono break-all text-muted-foreground">{where}</span>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{/if}
</article>
