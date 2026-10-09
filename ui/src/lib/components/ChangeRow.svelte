<script lang="ts">
	import type { SemanticChange } from '#lib/types.ts';
	import { fileHref, idHref } from '#lib/format.ts';
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

<article class="change" class:person>
	<button class="head ghost" onclick={() => (expanded = !expanded)} aria-expanded={expanded}>
		<span class="marker" title={person ? 'needs a person' : 'informational'}>{person ? '●' : '○'}</span>
		<span class="title"><Inline text={change.title} /></span>
		<span class="badges">
			<KindBadge kind={change.kind} label={change.kindLabel} />
			{#if change.hints.intentMismatch}<Badge tone="signal">outside intent</Badge>{/if}
			{#if change.hints.rulesOfTheGame}<Badge tone="signal">rules of the game</Badge>{/if}
		</span>
	</button>
	<p class="why"><Inline text={change.whyItMatters} /></p>
	{#if expanded}
		<div class="details">
			<dl>
				<dt>Subject</dt>
				<dd><a href={idHref(change.subject)} class="mono break">{change.subject}</a></dd>
				<dt>Subkind</dt>
				<dd class="mono">{change.subkind}</dd>
				{#if change.component}
					<dt>Component</dt>
					<dd><a href={idHref(change.component)}>{change.component}</a></dd>
				{/if}
				<dt>Blast radius</dt>
				<dd>{change.hints.blastRadius} dependent files</dd>
				<dt>Confidence</dt>
				<dd>{change.hints.confidence}</dd>
				{#if change.hints.labels.length}
					<dt>Labels</dt>
					<dd class="row">{#each change.hints.labels as l (l)}<Badge tone="signal">{l}</Badge>{/each}</dd>
				{/if}
				{#if change.hints.novelty.length}
					<dt>New</dt>
					<dd>{change.hints.novelty.join(', ')}</dd>
				{/if}
				{#if change.stats}
					<dt>Size</dt>
					<dd>
						<span class="add">+{change.stats.linesAdded}</span>
						<span class="del">−{change.stats.linesRemoved}</span>
						in {change.stats.files} files
					</dd>
				{/if}
				<dt>Row id</dt>
				<dd class="mono faint break">{change.id}</dd>
			</dl>
			{#if change.locations.length}
				<ul class="locations">
					{#each change.locations as l, i (i)}
						<li>
							<Badge tone="faint">{l.side}</Badge>
							{#if l.side === 'head'}
								<a class="mono" href={fileHref(l.file, l.lines[0], l.lines[1])}>{l.file}:{l.lines[0]}{l.lines[1] !== l.lines[0] ? `–${l.lines[1]}` : ''}</a>
							{:else}
								<span class="mono">{l.file}:{l.lines[0]}{l.lines[1] !== l.lines[0] ? `–${l.lines[1]}` : ''}</span>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{/if}
</article>

<style>
	.change {
		border-bottom: 1px solid var(--line);
		padding: var(--space-3) var(--space-4);
		display: grid;
		gap: var(--space-1);
	}
	.change:last-child {
		border-bottom: none;
	}
	.change.person {
		box-shadow: inset 3px 0 0 var(--signal);
	}
	.head {
		all: unset;
		display: flex;
		align-items: baseline;
		gap: var(--space-2);
		cursor: pointer;
		flex-wrap: wrap;
	}
	.head:focus-visible {
		outline: 2px solid var(--focus);
	}
	.marker {
		color: var(--ink-faint);
		width: 12px;
	}
	.person .marker {
		color: var(--signal);
	}
	.title {
		font-weight: 600;
		flex: 1 1 320px;
		overflow-wrap: anywhere;
	}
	.badges {
		display: inline-flex;
		gap: var(--space-1);
		flex-wrap: wrap;
	}
	.why {
		color: var(--ink-muted);
		padding-left: 20px;
		overflow-wrap: anywhere;
	}
	.details {
		padding: var(--space-2) 0 0 20px;
		display: grid;
		gap: var(--space-3);
	}
	dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 4px var(--space-4);
		margin: 0;
		font-size: 13px;
	}
	dt {
		color: var(--ink-faint);
	}
	dd {
		margin: 0;
		min-width: 0;
	}
	.locations {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: 4px;
		font-size: 12.5px;
	}
	.locations li {
		display: flex;
		gap: var(--space-2);
		align-items: center;
	}
	.add {
		color: var(--add);
	}
	.del {
		color: var(--del);
	}
</style>
