<script lang="ts">
	import type { ReportAnswer } from '#lib/types.ts';
	import { download, fileHref } from '#lib/format.ts';
	import Card from './Card.svelte';
	import ChangeRow from './ChangeRow.svelte';
	import ClassificationView from './ClassificationView.svelte';
	import Copy from './Copy.svelte';
	import Inline from './Inline.svelte';
	import Empty from './Empty.svelte';
	import Stat from './Stat.svelte';
	import Tabs from './Tabs.svelte';

	let { answer }: { answer: ReportAnswer } = $props();
	const r = $derived(answer.report);
	let tab = $state('changes');
	let expandAll = $state(false);
	const people = $derived(r.changes.filter((c) => c.hints.needsPerson).length);
</script>

<div class="stack">
	<div class="stats">
		<Stat label="Changes in meaning" value={r.summary.meaningChanges} hint="{r.textStats.linesAdded + r.textStats.linesRemoved} lines in {r.textStats.files} files" />
		<Stat label="Need a person" value={r.summary.needsAttention} tone={r.summary.needsAttention ? 'signal' : undefined} />
		<Stat label="Secrets" value={r.summary.secrets} tone={r.summary.secrets ? 'del' : undefined} />
		<Stat label="New rule violations" value={r.summary.newRuleViolations} tone={r.summary.newRuleViolations ? 'del' : undefined} />
		<Stat label="Outside the intent" value={r.summary.intentMismatches} tone={r.summary.intentMismatches ? 'signal' : undefined} />
	</div>

	{#if answer.classification}
		<Card title="Lane" subtitle="From onus.yaml's lane policy and the floors no policy can lower; without a submission, the agent's record and scope are not known.">
			<ClassificationView classification={answer.classification} />
		</Card>
	{/if}

	<Card pad={false}>
		{#snippet actions()}
			<span class="muted small mono">{r.base} → {r.head}</span>
		{/snippet}
		<div class="tabs-pad">
			<Tabs
				bind:value={tab}
				tabs={[
					{ id: 'changes', label: 'Changes', count: r.changes.length },
					{ id: 'violations', label: 'Rule violations', count: r.ruleViolations.length },
					{ id: 'checklist', label: 'Checklist', count: answer.checklist.length },
					{ id: 'structure', label: 'Structure', count: r.structure.movedFiles.length + r.structure.formattingOnly.length },
					{ id: 'markdown', label: 'Markdown' },
					{ id: 'json', label: 'JSON' }
				]}
			/>
		</div>
		{#if tab === 'changes'}
			{#if r.intentCheck}
				<div class="intent">
					<strong>Stated intent:</strong> {r.intentCheck.stated}
					{#if r.intentCheck.mismatches.length}
						<span class="signal-ink">· {r.intentCheck.mismatches.length} rows outside it</span>
					{/if}
				</div>
			{/if}
			{#if r.changes.length === 0}
				<div class="pad"><Empty title="No changes in meaning">The two sides mean the same thing: only formatting, comments or moves differ, if anything.</Empty></div>
			{:else}
				<div class="spread toolbar">
					<span class="muted small">{people} of {r.changes.length} need a person · ranked by what deserves attention</span>
					<button class="small ghost" onclick={() => (expandAll = !expandAll)}>{expandAll ? 'Collapse all' : 'Expand all'}</button>
				</div>
				{#each r.changes as c (c.id)}
					<ChangeRow change={c} open={expandAll} />
				{/each}
			{/if}
		{:else if tab === 'violations'}
			{#if r.ruleViolations.length === 0}
				<div class="pad"><Empty title="No new boundary-rule violations" /></div>
			{:else}
				{#each r.ruleViolations as c (c.id)}
					<ChangeRow change={c} />
				{/each}
			{/if}
		{:else if tab === 'checklist'}
			<div class="pad">
				{#if answer.checklist.length === 0}
					<Empty title="Nothing to follow up">None of the checked facts needs a follow-up. Onus does not check logic or whether tests pass.</Empty>
				{:else}
					<ul class="checklist">
						{#each answer.checklist as item, i (i)}<li><Inline text={item} /></li>{/each}
					</ul>
				{/if}
			</div>
		{:else if tab === 'structure'}
			<div class="pad stack">
				{#if r.structure.movedFiles.length}
					<div class="stack tight">
						<h3>Moved files</h3>
						<ul class="mono small">
							{#each r.structure.movedFiles as m (m.from)}<li>{m.from} → <a href={fileHref(m.to)}>{m.to}</a></li>{/each}
						</ul>
					</div>
				{/if}
				{#if r.structure.formattingOnly.length}
					<div class="stack tight">
						<h3>Formatting only</h3>
						<ul class="mono small">
							{#each r.structure.formattingOnly as f (f)}<li><a href={fileHref(f)}>{f}</a></li>{/each}
						</ul>
					</div>
				{/if}
				{#if r.mapDiagnostics.length}
					<div class="stack tight">
						<h3>Map notes</h3>
						<ul class="small">
							{#each r.mapDiagnostics as d, i (i)}<li>{d.file ? `${d.file}: ` : ''}{d.message}</li>{/each}
						</ul>
					</div>
				{/if}
				{#if !r.structure.movedFiles.length && !r.structure.formattingOnly.length && !r.mapDiagnostics.length}
					<Empty title="No moves or formatting-only files" />
				{/if}
			</div>
		{:else if tab === 'markdown'}
			<div class="pad stack tight">
				<div class="row end">
					<Copy text={answer.markdown} />
					<button class="small" onclick={() => download('onus-report.md', answer.markdown, 'text/markdown')}>Download</button>
				</div>
				<pre>{answer.markdown}</pre>
			</div>
		{:else}
			<div class="pad stack tight">
				<div class="row end">
					<button class="small" onclick={() => download('onus-report.json', JSON.stringify(r, null, 2))}>Download</button>
				</div>
				<pre>{JSON.stringify(r, null, 2)}</pre>
			</div>
		{/if}
	</Card>
</div>

<style>
	.tabs-pad {
		padding: var(--space-2) var(--space-4) 0;
	}
	.pad {
		padding: var(--space-4);
	}
	.toolbar {
		padding: var(--space-2) var(--space-4);
		border-bottom: 1px solid var(--line);
	}
	.intent {
		padding: var(--space-3) var(--space-4);
		border-bottom: 1px solid var(--line);
		background: var(--sunken);
		font-size: 13px;
	}
	.signal-ink {
		color: var(--signal-ink);
	}
	.checklist {
		margin: 0;
		padding-left: 1.2em;
		display: grid;
		gap: var(--space-2);
	}
	ul {
		margin: 0;
		padding-left: 1.2em;
	}
	pre {
		max-height: 60vh;
	}
</style>
