<script lang="ts">
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import BarList from '#lib/components/BarList.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Field from '#lib/components/Field.svelte';
	import LaneBadge from '#lib/components/LaneBadge.svelte';
	import NativeSelect from '#lib/components/NativeSelect.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { ago, idHref, percent } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Lane, Outcomes, Tally } from '#lib/types.ts';
	import Activity from '@lucide/svelte/icons/activity';
	import GitMerge from '@lucide/svelte/icons/git-merge';
	import SearchCheck from '@lucide/svelte/icons/search-check';
	import Siren from '@lucide/svelte/icons/siren';
	import Undo2 from '@lucide/svelte/icons/undo-2';
	import UserRound from '@lucide/svelte/icons/user-round';
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';

	const data = new Task<Outcomes>();
	const action = new Task<Outcomes>();
	const q = page.url.searchParams;

	let tab = $state('record');
	let rec = $state({
		change: q.get('change') ?? '',
		agent: q.get('agent') ?? '',
		lane: (q.get('lane') ?? 'judge') as Lane,
		result: 'merged',
		verdict: q.get('verdict') ?? '',
		judge: q.get('judge') ?? '',
		commit: q.get('commit') ?? '',
		audited: false,
		missed: false
	});
	let incident = $state({ change: '', involved: '', note: '' });
	let since = $state('');

	onMount(() => data.run(() => api<Outcomes>('outcomes')));

	async function act(name: string, body: unknown, done: string) {
		const v = await action.run(() => api<Outcomes>(name, body));
		if (v) {
			data.value = v;
			toast.success(done);
		}
		return v;
	}

	async function record(e: SubmitEvent) {
		e.preventDefault();
		if (await act('outcomes.record', rec, `Recorded ${rec.result} for ${rec.change}`)) rec.change = '';
	}

	async function addIncident(e: SubmitEvent) {
		e.preventDefault();
		const involved = incident.involved.split(/[\n,]/).map((s) => s.trim()).filter(Boolean);
		if (await act('outcomes.incident', { change: incident.change, involved, note: incident.note }, `Incident recorded against ${incident.change}`)) {
			incident = { change: '', involved: '', note: '' };
		}
	}

	async function reverts(e: SubmitEvent) {
		e.preventDefault();
		const v = await action.run(() => api<Outcomes>('outcomes.ingestReverts', { since }));
		if (v) {
			data.value = v;
			toast.success(`${v.ingested?.found ?? 0} reverts found, ${v.ingested?.recorded ?? 0} recorded`);
		}
	}

	const tallies = (m: Record<string, Tally>) => Object.entries(m).sort((a, b) => b[1].changes - a[1].changes);
	const changes = $derived([...new Set((data.value?.records ?? []).map((r) => r.change))].sort());
	const resultTone = (r: string) => (r === 'merged' ? 'add' : r === 'incident' || r === 'reverted' || r === 'rolled-back' ? 'del' : 'faint');
	const laneColor: Record<string, string> = { 'auto-merge': 'bg-success', judge: 'bg-info', human: 'bg-signal', blocked: 'bg-destructive' };
	const laneTotal = $derived(Object.values(data.value?.summary.byLane ?? {}).reduce((a, b) => a + b, 0));
</script>

<PageHead title="Outcomes" guide="lanes">
	What happened to changes after they left the lanes: the track record that decides whether an agent setup may merge on its own, and whether the automatic lanes can be trusted.
</PageHead>

{#if data.error}
	<ErrorBox error={data.error} />
{:else if !data.value}
	<div class="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">{#each Array(6) as _, i (i)}<Skeleton class="h-24 rounded-xl" />{/each}</div>
{:else}
	{@const s = data.value.summary}
	<div class="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
		<Stat label="Changes" value={s.total.changes} icon={Activity} />
		<Stat label="Merged" value={s.total.merged} icon={GitMerge} />
		<Stat label="Reverted or rolled back" value={s.total.reverted} icon={Undo2} tone={s.total.reverted ? 'del' : undefined} />
		<Stat label="Incidents" value={s.total.incidents} icon={Siren} tone={s.total.incidents ? 'del' : undefined} />
		<Stat label="Human-lane share" value={percent(s.humanShare)} icon={UserRound} hint="the review budget" />
		<Stat label="Audit miss rate" value={percent(s.auditMissRate)} icon={SearchCheck} hint="audits that found a miss" tone={s.auditMissRate ? 'signal' : undefined} />
	</div>

	<div class="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
		<Card title="By agent setup" subtitle="Each setup's record decides whether it may auto-merge." pad={false}>
			{#if Object.keys(s.byAgent).length}
				<Table.Root>
					<Table.Header><Table.Row><Table.Head class="pl-4">Setup</Table.Head><Table.Head class="text-right">Changes</Table.Head><Table.Head class="text-right">Merged</Table.Head><Table.Head class="text-right">Reverted</Table.Head><Table.Head class="text-right">Incidents</Table.Head><Table.Head class="text-right">Audited</Table.Head><Table.Head class="pr-4 text-right">Missed</Table.Head></Table.Row></Table.Header>
					<Table.Body>
						{#each tallies(s.byAgent) as [agent, t] (agent)}
							<Table.Row>
								<Table.Cell class="pl-4 font-mono text-xs">{agent}</Table.Cell>
								<Table.Cell class="text-right tabular-nums">{t.changes}</Table.Cell>
								<Table.Cell class="text-right tabular-nums">{t.merged}</Table.Cell>
								<Table.Cell class="text-right tabular-nums {t.reverted ? 'text-destructive' : ''}">{t.reverted}</Table.Cell>
								<Table.Cell class="text-right tabular-nums {t.incidents ? 'text-destructive' : ''}">{t.incidents}</Table.Cell>
								<Table.Cell class="text-right tabular-nums">{t.audited}</Table.Cell>
								<Table.Cell class="pr-4 text-right tabular-nums {t.missed ? 'text-signal-foreground' : ''}">{t.missed}</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			{:else}<div class="p-4"><Empty title="No outcomes yet">Record one below, or with <code>onus outcomes record</code> in CI.</Empty></div>{/if}
		</Card>
		<div class="grid content-start gap-4">
			<Card title="By lane">
				{#if laneTotal}
					<div class="flex h-3 overflow-hidden rounded-full bg-muted">
						{#each Object.entries(s.byLane) as [lane, n] (lane)}<div class="{laneColor[lane] ?? 'bg-faint'} h-full" style="width: {(n / laneTotal) * 100}%"></div>{/each}
					</div>
					<ul class="mt-3 grid gap-2 text-sm">
						{#each Object.entries(s.byLane) as [lane, n] (lane)}
							<li class="flex items-center gap-2"><span class="size-2.5 rounded-full {laneColor[lane] ?? 'bg-faint'}"></span><LaneBadge lane={lane as Lane} /><span class="ml-auto tabular-nums">{n}</span><span class="w-12 text-right text-xs text-muted-foreground">{percent(n / laneTotal)}</span></li>
						{/each}
					</ul>
				{:else}<p class="text-sm text-muted-foreground">No changes on record.</p>{/if}
			</Card>
			<Card title="Held-out backlog" subtitle="What incidents involved: where the next held-out tests should go. Onus does not write tests.">
				{#if data.value.backlog.length}
					<BarList items={data.value.backlog.map((b) => ({ label: b.target, value: b.incidents, href: idHref(b.target) }))} />
				{:else}<p class="text-sm text-muted-foreground">No incidents on record.</p>{/if}
			</Card>
			{#if Object.keys(s.byJudge).length}
				<Card title="By judge configuration" pad={false}>
					<Table.Root>
						<Table.Body>
							{#each tallies(s.byJudge) as [j, t] (j)}
								<Table.Row><Table.Cell class="pl-4 font-mono text-xs">{j}</Table.Cell><Table.Cell class="text-right text-xs">{t.changes} changes · {t.reverted} reverted · {t.missed} missed</Table.Cell></Table.Row>
							{/each}
						</Table.Body>
					</Table.Root>
				</Card>
			{/if}
		</div>
	</div>

	<Card pad={false}>
		<div class="border-b px-4 py-3">
			<Tabs bind:value={tab} tabs={[{ id: 'record', label: 'Record an outcome' }, { id: 'incident', label: 'Record an incident' }, { id: 'reverts', label: 'Find reverts' }]} />
		</div>
		<div class="p-4">
			{#if tab === 'record'}
				<form class="grid gap-4" onsubmit={record}>
					<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
						<Field label="Change"><Input bind:value={rec.change} placeholder="acme/shop#42 or a commit" required /></Field>
						<Field label="Agent setup"><Input class="font-mono" bind:value={rec.agent} placeholder="claude-code/sonnet/default" required /></Field>
						<Field label="Lane"><NativeSelect bind:value={rec.lane}>{#each ['auto-merge', 'judge', 'human', 'blocked'] as l (l)}<option>{l}</option>{/each}</NativeSelect></Field>
						<Field label="Result"><NativeSelect bind:value={rec.result}>{#each data.value.results as r (r)}<option>{r}</option>{/each}</NativeSelect></Field>
						<Field label="Verdict"><Input bind:value={rec.verdict} placeholder="approve" /></Field>
						<Field label="Judge"><Input class="font-mono" bind:value={rec.judge} /></Field>
						<Field label="Commit it landed as"><Input class="font-mono" bind:value={rec.commit} /></Field>
					</div>
					<div class="flex flex-wrap items-center gap-5 text-sm">
						<label class="flex items-center gap-2"><Checkbox bind:checked={rec.audited} />A person audited it</label>
						<label class="flex items-center gap-2"><Checkbox bind:checked={rec.missed} />The audit found a miss</label>
					</div>
					<div><Button type="submit" disabled={action.running}>Record</Button></div>
				</form>
			{:else if tab === 'incident'}
				<form class="grid gap-4" onsubmit={addIncident}>
					<div class="grid gap-3 md:grid-cols-2">
						<Field label="Change on record">
							<NativeSelect bind:value={incident.change} required>
								<option value="" disabled>Choose…</option>
								{#each changes as c (c)}<option>{c}</option>{/each}
							</NativeSelect>
						</Field>
						<Field label="What it involved" hint="Components or symbol ids, one per line."><Textarea rows={2} class="font-mono text-xs" bind:value={incident.involved} /></Field>
					</div>
					<Field label="What happened"><Input bind:value={incident.note} required /></Field>
					<div><Button type="submit" disabled={action.running || !incident.change}><Siren />Record the incident</Button></div>
				</form>
			{:else}
				<form class="flex flex-wrap items-end gap-3" onsubmit={reverts}>
					<Field label="Since (a ref, optional)"><Input bind:value={since} class="w-56 font-mono" /></Field>
					<Button type="submit" disabled={action.running}><Undo2 />Find reverts in git history</Button>
				</form>
			{/if}
			{#if action.error}<div class="mt-3"><ErrorBox error={action.error} /></div>{/if}
		</div>
	</Card>

	<Card title="Records" subtitle={data.value.file} pad={false}>
		{#if data.value.records.length}
			<Table.Root>
				<Table.Header><Table.Row><Table.Head class="pl-4">When</Table.Head><Table.Head>Change</Table.Head><Table.Head>Agent setup</Table.Head><Table.Head>Lane</Table.Head><Table.Head>Verdict</Table.Head><Table.Head>Result</Table.Head><Table.Head>Audit</Table.Head><Table.Head class="pr-4">Note</Table.Head></Table.Row></Table.Header>
				<Table.Body>
					{#each [...data.value.records].reverse() as o, i (i)}
						<Table.Row>
							<Table.Cell class="pl-4 text-xs whitespace-nowrap text-muted-foreground">{ago(o.at)}</Table.Cell>
							<Table.Cell class="font-mono text-xs">{o.change}</Table.Cell>
							<Table.Cell class="font-mono text-xs">{o.agent}</Table.Cell>
							<Table.Cell><LaneBadge lane={o.lane} /></Table.Cell>
							<Table.Cell class="text-xs">{o.verdict ?? ''}</Table.Cell>
							<Table.Cell><Badge tone={resultTone(o.result)}>{o.result}</Badge></Table.Cell>
							<Table.Cell class="text-xs">{o.audited ? (o.missed ? 'missed' : 'audited') : ''}</Table.Cell>
							<Table.Cell class="pr-4 text-xs">{o.note ?? ''}</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
		{:else}
			<div class="p-4"><Empty title="No outcomes recorded" /></div>
		{/if}
	</Card>
{/if}
