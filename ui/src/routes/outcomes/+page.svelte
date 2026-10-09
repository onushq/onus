<script lang="ts">
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import LaneBadge from '#lib/components/LaneBadge.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { ago, idHref, percent } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Lane, Outcomes, Tally } from '#lib/types.ts';
	import { onMount } from 'svelte';

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

	async function act(name: string, body: unknown) {
		const v = await action.run(() => api<Outcomes>(name, body));
		if (v) data.value = v;
		return v;
	}

	async function record(e: SubmitEvent) {
		e.preventDefault();
		if (await act('outcomes.record', rec)) rec.change = '';
	}

	async function addIncident(e: SubmitEvent) {
		e.preventDefault();
		const involved = incident.involved.split(/[\n,]/).map((s) => s.trim()).filter(Boolean);
		if (await act('outcomes.incident', { change: incident.change, involved, note: incident.note })) {
			incident = { change: '', involved: '', note: '' };
		}
	}

	const tallies = (m: Record<string, Tally>) => Object.entries(m).sort((a, b) => b[1].changes - a[1].changes);
	const changes = $derived([...new Set((data.value?.records ?? []).map((r) => r.change))].sort());
	const resultTone = (r: string) =>
		r === 'merged' ? 'add' : r === 'incident' || r === 'reverted' || r === 'rolled-back' ? 'del' : 'faint';
</script>

<PageHead title="Outcomes" guide="lanes">
	What happened to changes after they left the lanes: the track record that decides whether an agent setup may
	merge on its own, and whether the automatic lanes can be trusted.
</PageHead>

{#if data.error}
	<ErrorBox error={data.error} />
{:else if !data.value}
	<Loading />
{:else}
	{@const s = data.value.summary}
	<div class="stack">
		<div class="stats">
			<Stat label="Changes" value={s.total.changes} />
			<Stat label="Merged" value={s.total.merged} />
			<Stat label="Reverted or rolled back" value={s.total.reverted} tone={s.total.reverted ? 'del' : undefined} />
			<Stat label="Incidents" value={s.total.incidents} tone={s.total.incidents ? 'del' : undefined} />
			<Stat label="Human-lane share" value={percent(s.humanShare)} hint="the review budget" />
			<Stat label="Audit miss rate" value={percent(s.auditMissRate)} hint="audits of automatic approvals that found a miss" tone={s.auditMissRate ? 'signal' : undefined} />
		</div>

		<div class="grid-2">
			<Card title="By agent setup" pad={false}>
				{#if Object.keys(s.byAgent).length}
					<table class="data">
						<thead><tr><th>Setup</th><th class="num">Changes</th><th class="num">Merged</th><th class="num">Reverted</th><th class="num">Incidents</th><th class="num">Audited</th><th class="num">Missed</th></tr></thead>
						<tbody>
							{#each tallies(s.byAgent) as [agent, t] (agent)}
								<tr><td class="mono small">{agent}</td><td class="num">{t.changes}</td><td class="num">{t.merged}</td><td class="num">{t.reverted}</td><td class="num">{t.incidents}</td><td class="num">{t.audited}</td><td class="num">{t.missed}</td></tr>
							{/each}
						</tbody>
					</table>
				{:else}<div style="padding: 16px"><Empty title="No outcomes yet" /></div>{/if}
			</Card>
			<div class="stack">
				<Card title="By judge configuration" pad={false}>
					{#if Object.keys(s.byJudge).length}
						<table class="data">
							<thead><tr><th>Judge</th><th class="num">Changes</th><th class="num">Merged</th><th class="num">Reverted</th><th class="num">Missed</th></tr></thead>
							<tbody>
								{#each tallies(s.byJudge) as [j, t] (j)}
									<tr><td class="mono small">{j}</td><td class="num">{t.changes}</td><td class="num">{t.merged}</td><td class="num">{t.reverted}</td><td class="num">{t.missed}</td></tr>
								{/each}
							</tbody>
						</table>
					{:else}<div style="padding: 16px" class="muted small">No judged changes yet.</div>{/if}
				</Card>
				<Card title="By lane">
					<div class="row">
						{#each Object.entries(s.byLane) as [lane, n] (lane)}<span class="row"><LaneBadge lane={lane as Lane} /> {n}</span>{:else}<span class="muted small">–</span>{/each}
					</div>
				</Card>
			</div>
		</div>

		<Card title="Held-out backlog" subtitle="What incidents involved, most often first: where the next held-out tests should go. Onus does not write tests.">
			{#if data.value.backlog.length}
				<ul class="plain">
					{#each data.value.backlog as b (b.target)}
						<li><Badge tone="del">{b.incidents}</Badge> <a class="mono small" href={idHref(b.target)}>{b.target}</a></li>
					{/each}
				</ul>
			{:else}<p class="muted small">No incidents on record.</p>{/if}
		</Card>

		<Card pad={false}>
			<div style="padding: 8px 16px 0">
				<Tabs bind:value={tab} tabs={[{ id: 'record', label: 'Record an outcome' }, { id: 'incident', label: 'Record an incident' }, { id: 'reverts', label: 'Find reverts' }]} />
			</div>
			<div style="padding: 16px">
				{#if tab === 'record'}
					<form class="stack" onsubmit={record}>
						<div class="grid-3">
							<label class="field">Change<input bind:value={rec.change} placeholder="acme/shop#42 or a commit" required /></label>
							<label class="field">Agent setup<input class="mono" bind:value={rec.agent} placeholder="claude-code/sonnet/default" required /></label>
							<label class="field">Lane
								<select bind:value={rec.lane}>{#each ['auto-merge', 'judge', 'human', 'blocked'] as l (l)}<option>{l}</option>{/each}</select>
							</label>
							<label class="field">Result
								<select bind:value={rec.result}>{#each data.value.results as r (r)}<option>{r}</option>{/each}</select>
							</label>
							<label class="field">Verdict<input bind:value={rec.verdict} placeholder="approve" /></label>
							<label class="field">Judge<input class="mono" bind:value={rec.judge} /></label>
							<label class="field">Commit it landed as<input class="mono" bind:value={rec.commit} /></label>
						</div>
						<div class="row">
							<label class="check"><input type="checkbox" bind:checked={rec.audited} /> A person audited it</label>
							<label class="check"><input type="checkbox" bind:checked={rec.missed} /> The audit found a miss</label>
						</div>
						<div class="row"><button class="primary" type="submit" disabled={action.running}>Record</button></div>
					</form>
				{:else if tab === 'incident'}
					<form class="stack" onsubmit={addIncident}>
						<div class="grid-2">
							<label class="field">Change on record
								<select bind:value={incident.change} required>
									<option value="" disabled>Choose…</option>
									{#each changes as c (c)}<option>{c}</option>{/each}
								</select>
							</label>
							<label class="field">What it involved (components or symbol ids, one per line)<textarea rows="2" bind:value={incident.involved}></textarea></label>
						</div>
						<label class="field">What happened<input bind:value={incident.note} required /></label>
						<div class="row"><button class="primary" type="submit" disabled={action.running || !incident.change}>Record the incident</button></div>
					</form>
				{:else}
					<form class="row" onsubmit={(e) => { e.preventDefault(); act('outcomes.ingestReverts', { since }); }}>
						<input bind:value={since} placeholder="since (a ref, optional)" />
						<button class="primary" type="submit" disabled={action.running}>Find reverts in git history</button>
						{#if data.value.ingested}<span class="muted small">{data.value.ingested.found} reverts found, {data.value.ingested.recorded} recorded</span>{/if}
					</form>
				{/if}
				{#if action.error}<div style="margin-top: 12px"><ErrorBox error={action.error} /></div>{/if}
			</div>
		</Card>

		<Card title="Records" subtitle={data.value.file} pad={false}>
			{#if data.value.records.length}
				<div class="table-wrap">
					<table class="data">
						<thead><tr><th>When</th><th>Change</th><th>Agent setup</th><th>Lane</th><th>Verdict</th><th>Result</th><th>Audit</th><th>Note</th></tr></thead>
						<tbody>
							{#each [...data.value.records].reverse() as o, i (i)}
								<tr>
									<td class="small faint nowrap">{ago(o.at)}</td>
									<td class="mono small">{o.change}</td>
									<td class="mono small">{o.agent}</td>
									<td><LaneBadge lane={o.lane} /></td>
									<td class="small">{o.verdict ?? ''}</td>
									<td><Badge tone={resultTone(o.result)}>{o.result}</Badge></td>
									<td class="small">{o.audited ? (o.missed ? 'missed' : 'audited') : ''}</td>
									<td class="small">{o.note ?? ''}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{:else}
				<div style="padding: 16px"><Empty title="No outcomes recorded">Record one above, or with <code>onus outcomes record</code> in CI.</Empty></div>
			{/if}
		</Card>
	</div>
{/if}

<style>
	.plain {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: 6px;
	}
</style>
