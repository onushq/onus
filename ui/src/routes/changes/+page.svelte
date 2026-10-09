<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import { app } from '#lib/app.svelte.ts';
	import Card from '#lib/components/Card.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import RefPicker from '#lib/components/RefPicker.svelte';
	import ReportView from '#lib/components/ReportView.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { Task } from '#lib/task.svelte.ts';
	import type { ReportAnswer } from '#lib/types.ts';
	import { onMount } from 'svelte';

	const params = page.url.searchParams;
	let mode = $state(params.get('head') ? 'refs' : 'worktree');
	let base = $state(params.get('base') ?? '');
	let head = $state(params.get('head') ?? 'HEAD');
	let checkBase = $state('HEAD');
	let intent = $state('');
	let showIntent = $state(false);

	const worktree = new Task<ReportAnswer>();
	const report = new Task<ReportAnswer>();

	onMount(() => {
		app.loadRefs().then((r) => {
			if (!base) base = r?.default ?? 'main';
		});
	});

	function check() {
		const b = checkBase || 'HEAD';
		worktree.run(() => api<ReportAnswer>('check', { base: b }));
	}

	function compare(e?: SubmitEvent) {
		e?.preventDefault();
		const b = base.trim();
		const h = head.trim();
		if (!b || !h) return;
		goto(`/changes?base=${encodeURIComponent(b)}&head=${encodeURIComponent(h)}`, { replace: true, reset: false });
		report.run(() => api<ReportAnswer>('report', { base: b, head: h, intent: intent.trim() || undefined }));
	}

	// Opening with ?base=&head= reports right away; the worktree is checked
	// when the page opens on it.
	onMount(() => {
		if (params.get('head')) compare();
		else check();
	});
</script>

<PageHead title="Changes" guide="reports">
	What a change means: new dependencies, contract changes, data leaving the system, rules broken. Computed from
	the code, never generated.
</PageHead>

<div class="stack">
	<Tabs
		bind:value={mode}
		tabs={[
			{ id: 'worktree', label: 'Uncommitted work', count: app.status?.dirty.length },
			{ id: 'refs', label: 'Compare refs' }
		]}
	/>

	{#if mode === 'worktree'}
		<Card>
			<div class="spread">
				<p class="muted">
					The files on disk, saved but not committed, against
					<input class="inline" bind:value={checkBase} aria-label="Base ref" />.
					This is what <code>onus_check</code> gives an agent before it commits.
				</p>
				<button class="primary" onclick={check} disabled={worktree.running}>Check again</button>
			</div>
		</Card>
		{#if worktree.running && !worktree.value}
			<Loading label="Reading the change…" />
		{:else if worktree.error}
			<ErrorBox error={worktree.error} />
		{:else if worktree.value}
			<div class:stale={worktree.running}><ReportView answer={worktree.value} /></div>
		{/if}
	{:else}
		<Card>
			<form class="stack" onsubmit={compare}>
				<div class="refs">
					<RefPicker id="base" label="Base" bind:value={base} />
					<span class="arrow">→</span>
					<RefPicker id="head" label="Head" bind:value={head} />
					<button class="primary" type="submit" disabled={report.running || !base || !head}>
						{report.running ? 'Reporting…' : 'Report'}
					</button>
				</div>
				<button type="button" class="ghost small" onclick={() => (showIntent = !showIntent)}>
					{showIntent ? 'Hide the intent' : 'Check against a stated intent'}
				</button>
				{#if showIntent}
					<label class="field"><span>Intent: YAML, or a pull request body with an <code>onus-intent</code> block. Rows outside it rank first.</span>
						<textarea rows="6" bind:value={intent} placeholder={'summary: Text customers when their order ships\ntouches: [notifications]\nexternals: [Acme SMS]'}></textarea>
					</label>
				{/if}
			</form>
		</Card>
		{#if report.running && !report.value}
			<Loading label="Extracting both refs and comparing their maps…" />
		{:else if report.error}
			<ErrorBox error={report.error} />
		{:else if report.value}
			{#if report.value.notes?.length}
				<p class="muted small">{report.value.notes.join(' · ')}</p>
			{/if}
			<div class:stale={report.running}><ReportView answer={report.value} /></div>
			<p class="small muted">
				To send this change through the lanes with its evidence, open
				<a href="/lanes?base={encodeURIComponent(base)}&head={encodeURIComponent(head)}">Lanes &amp; judge</a>.
			</p>
		{/if}
	{/if}
</div>

<style>
	.refs {
		display: flex;
		align-items: flex-end;
		gap: var(--space-3);
		flex-wrap: wrap;
	}
	.refs :global(label) {
		flex: 1 1 220px;
	}
	.arrow {
		padding-bottom: 6px;
		color: var(--ink-faint);
	}
	.inline {
		height: 26px;
		width: 120px;
		font-family: var(--font-mono);
		font-size: 12.5px;
	}
	.stale {
		opacity: 0.55;
		transition: opacity 0.2s;
	}
</style>
