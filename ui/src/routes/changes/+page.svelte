<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { api } from '#lib/api.ts';
	import { app } from '#lib/app.svelte.ts';
	import Card from '#lib/components/Card.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Field from '#lib/components/Field.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import RefPicker from '#lib/components/RefPicker.svelte';
	import ReportView from '#lib/components/ReportView.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { Task } from '#lib/task.svelte.ts';
	import type { ReportAnswer } from '#lib/types.ts';
	import ArrowRight from '@lucide/svelte/icons/arrow-right';
	import Gavel from '@lucide/svelte/icons/gavel';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
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
		if (params.get('head')) compare();
		else check();
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
		mode = 'refs';
		goto(`/changes?base=${encodeURIComponent(b)}&head=${encodeURIComponent(h)}`, { replace: true, reset: false });
		report.run(() => api<ReportAnswer>('report', { base: b, head: h, intent: intent.trim() || undefined }));
	}
</script>

<PageHead title="Changes" guide="reports">
	What a change means: new dependencies, contract changes, data leaving the system, rules broken. Computed from the code, never generated.
</PageHead>

<Tabs
	bind:value={mode}
	tabs={[
		{ id: 'worktree', label: 'Uncommitted work', count: app.status?.dirty.length },
		{ id: 'refs', label: 'Compare refs' }
	]}
/>

{#if mode === 'worktree'}
	<Card>
		<div class="flex flex-wrap items-center justify-between gap-3">
			<p class="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
				The files on disk, saved but not committed, against
				<Input class="h-7 w-28 font-mono text-xs" bind:value={checkBase} aria-label="Base ref" />
				— what <code>onus_check</code> gives an agent before it commits.
			</p>
			<Button onclick={check} disabled={worktree.running}><RefreshCw class={worktree.running ? 'animate-spin' : ''} />Check again</Button>
		</div>
	</Card>
	{#if worktree.running && !worktree.value}
		<div class="grid grid-cols-2 gap-3 md:grid-cols-5">{#each Array(5) as _, i (i)}<Skeleton class="h-24 rounded-xl" />{/each}</div>
		<Skeleton class="h-64 rounded-xl" />
	{:else if worktree.error}
		<ErrorBox error={worktree.error} />
	{:else if worktree.value}
		<div class:opacity-60={worktree.running} class="transition-opacity"><ReportView answer={worktree.value} /></div>
	{/if}
{:else}
	<Card>
		<form class="grid gap-3" onsubmit={compare}>
			<div class="grid items-end gap-3 md:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]">
				<RefPicker id="base" label="Base" bind:value={base} />
				<ArrowRight class="mb-2 hidden size-4 text-muted-foreground md:block" />
				<RefPicker id="head" label="Head" bind:value={head} />
				<Button type="submit" disabled={report.running || !base || !head}>{report.running ? 'Reporting…' : 'Report'}</Button>
			</div>
			<div>
				<Button type="button" variant="link" size="sm" class="h-auto px-0" onclick={() => (showIntent = !showIntent)}>
					{showIntent ? 'Hide the intent' : 'Check against a stated intent'}
				</Button>
			</div>
			{#if showIntent}
				<Field hint="YAML, or a pull request body with an onus-intent block. Rows outside it rank first.">
					{#snippet label()}Intent{/snippet}
					<Textarea rows={5} class="font-mono text-xs" bind:value={intent} placeholder={'summary: Text customers when their order ships\ntouches: [notifications]\nexternals: [Acme SMS]'} />
				</Field>
			{/if}
		</form>
	</Card>
	{#if report.running && !report.value}
		<div class="grid grid-cols-2 gap-3 md:grid-cols-5">{#each Array(5) as _, i (i)}<Skeleton class="h-24 rounded-xl" />{/each}</div>
		<p class="text-sm text-muted-foreground">Extracting both refs and comparing their maps…</p>
	{:else if report.error}
		<ErrorBox error={report.error} />
	{:else if report.value}
		{#if report.value.notes?.length}<p class="text-xs text-muted-foreground">{report.value.notes.join(' · ')}</p>{/if}
		<div class:opacity-60={report.running} class="transition-opacity"><ReportView answer={report.value} /></div>
		<div class="flex items-center gap-3 rounded-xl border border-dashed p-4 text-sm">
			<Gavel class="size-4 text-muted-foreground" />
			<span class="flex-1 text-muted-foreground">Send this change through the lanes with its evidence, and let the judge verify it.</span>
			<Button variant="outline" size="sm" href="/lanes?base={encodeURIComponent(base)}&head={encodeURIComponent(head)}">Lanes &amp; judge<ArrowRight /></Button>
		</div>
	{/if}
{/if}
