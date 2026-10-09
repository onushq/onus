<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { download, fileHref } from '#lib/format.ts';
	import type { ReportAnswer } from '#lib/types.ts';
	import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
	import Download from '@lucide/svelte/icons/download';
	import FileDiff from '@lucide/svelte/icons/file-diff';
	import KeyRound from '@lucide/svelte/icons/key-round';
	import ShieldAlert from '@lucide/svelte/icons/shield-alert';
	import Target from '@lucide/svelte/icons/target';
	import UserRound from '@lucide/svelte/icons/user-round';
	import Card from './Card.svelte';
	import ChangeRow from './ChangeRow.svelte';
	import ClassificationView from './ClassificationView.svelte';
	import Copy from './Copy.svelte';
	import Empty from './Empty.svelte';
	import Inline from './Inline.svelte';
	import Stat from './Stat.svelte';
	import Tabs from './Tabs.svelte';

	let { answer }: { answer: ReportAnswer } = $props();
	const r = $derived(answer.report);
	let tab = $state('changes');
	let expandAll = $state(false);
	const people = $derived(r.changes.filter((c) => c.hints.needsPerson).length);
</script>

<div class="grid gap-4">
	<div class="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-5">
		<Stat
			label="Changes in meaning"
			value={r.summary.meaningChanges}
			icon={FileDiff}
			hint="{(r.textStats.linesAdded + r.textStats.linesRemoved).toLocaleString()} lines in {r.textStats.files} files"
		/>
		<Stat label="Need a person" value={r.summary.needsAttention} icon={UserRound} tone={r.summary.needsAttention ? 'signal' : undefined} />
		<Stat label="Secrets" value={r.summary.secrets} icon={KeyRound} tone={r.summary.secrets ? 'del' : undefined} />
		<Stat label="New rule violations" value={r.summary.newRuleViolations} icon={ShieldAlert} tone={r.summary.newRuleViolations ? 'del' : undefined} />
		<Stat label="Outside the intent" value={r.summary.intentMismatches} icon={Target} tone={r.summary.intentMismatches ? 'signal' : undefined} />
	</div>

	<div class="grid gap-4 {answer.classification ? 'xl:grid-cols-[minmax(0,1fr)_340px]' : ''}">
		<Card pad={false}>
			<div class="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-3">
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
				<span class="font-mono text-xs text-muted-foreground">{r.base} → {r.head}</span>
			</div>
			{#if tab === 'changes'}
				{#if r.intentCheck}
					<div class="border-b bg-muted/40 px-4 py-2.5 text-sm">
						<span class="text-muted-foreground">Stated intent:</span>
						{r.intentCheck.stated}
						{#if r.intentCheck.mismatches.length}
							<span class="font-medium text-signal-foreground">· {r.intentCheck.mismatches.length} rows outside it</span>
						{/if}
					</div>
				{/if}
				{#if r.changes.length === 0}
					<div class="p-4">
						<Empty title="No changes in meaning">The two sides mean the same thing: only formatting, comments or moves differ, if anything.</Empty>
					</div>
				{:else}
					<div class="flex items-center justify-between gap-3 border-b px-4 py-2 text-xs text-muted-foreground">
						<span>{people} of {r.changes.length} need a person · most important first</span>
						<Button variant="ghost" size="xs" onclick={() => (expandAll = !expandAll)}>
							<ChevronsUpDown />{expandAll ? 'Collapse all' : 'Expand all'}
						</Button>
					</div>
					{#each r.changes as c (c.id)}<ChangeRow change={c} open={expandAll} />{/each}
				{/if}
			{:else if tab === 'violations'}
				{#if r.ruleViolations.length === 0}
					<div class="p-4"><Empty title="No new boundary-rule violations" /></div>
				{:else}
					{#each r.ruleViolations as c (c.id)}<ChangeRow change={c} />{/each}
				{/if}
			{:else if tab === 'checklist'}
				<div class="p-4">
					{#if answer.checklist.length === 0}
						<Empty title="Nothing to follow up">None of the checked facts needs a follow-up. Onus does not check logic or whether tests pass.</Empty>
					{:else}
						<ul class="grid gap-2 text-sm">
							{#each answer.checklist as item, i (i)}
								<li class="flex gap-2"><span class="mt-1.5 size-1.5 shrink-0 rounded-full bg-signal"></span><span><Inline text={item} /></span></li>
							{/each}
						</ul>
					{/if}
				</div>
			{:else if tab === 'structure'}
				<div class="grid gap-5 p-4 text-sm">
					{#if r.structure.movedFiles.length}
						<div class="grid gap-2">
							<h3 class="font-medium">Moved files</h3>
							<ul class="grid gap-1 font-mono text-xs">
								{#each r.structure.movedFiles as m (m.from)}<li>{m.from} → <a class="hover:underline" href={fileHref(m.to)}>{m.to}</a></li>{/each}
							</ul>
						</div>
					{/if}
					{#if r.structure.formattingOnly.length}
						<div class="grid gap-2">
							<h3 class="font-medium">Formatting only</h3>
							<ul class="grid gap-1 font-mono text-xs">
								{#each r.structure.formattingOnly as f (f)}<li><a class="hover:underline" href={fileHref(f)}>{f}</a></li>{/each}
							</ul>
						</div>
					{/if}
					{#if r.mapDiagnostics.length}
						<div class="grid gap-2">
							<h3 class="font-medium">Map notes</h3>
							<ul class="grid gap-1 text-muted-foreground">
								{#each r.mapDiagnostics as d, i (i)}<li>{d.file ? `${d.file}: ` : ''}{d.message}</li>{/each}
							</ul>
						</div>
					{/if}
					{#if !r.structure.movedFiles.length && !r.structure.formattingOnly.length && !r.mapDiagnostics.length}
						<Empty title="No moves or formatting-only files" />
					{/if}
				</div>
			{:else if tab === 'markdown'}
				<div class="grid gap-3 p-4">
					<div class="flex justify-end gap-2">
						<Copy text={answer.markdown} />
						<Button variant="outline" size="sm" onclick={() => download('onus-report.md', answer.markdown, 'text/markdown')}><Download />Download</Button>
					</div>
					<pre class="max-h-[60vh]">{answer.markdown}</pre>
				</div>
			{:else}
				<div class="grid gap-3 p-4">
					<div class="flex justify-end">
						<Button variant="outline" size="sm" onclick={() => download('onus-report.json', JSON.stringify(r, null, 2))}><Download />Download</Button>
					</div>
					<pre class="max-h-[60vh]">{JSON.stringify(r, null, 2)}</pre>
				</div>
			{/if}
		</Card>

		{#if answer.classification}
			<Card title="Lane" subtitle="The lane policy and the floors no policy can lower. Without a submission the agent's record and scope are not known." class="self-start">
				<ClassificationView classification={answer.classification} />
			</Card>
		{/if}
	</div>
</div>
