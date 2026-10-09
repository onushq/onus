<script lang="ts">
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import { app } from '#lib/app.svelte.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import ChangeRow from '#lib/components/ChangeRow.svelte';
	import ClassificationView from '#lib/components/ClassificationView.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Inline from '#lib/components/Inline.svelte';
	import LaneBadge from '#lib/components/LaneBadge.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import RefPicker from '#lib/components/RefPicker.svelte';
	import { ago, download, short } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Classification, Judgment, LanesPolicy, Run, Submission } from '#lib/types.ts';
	import { onMount } from 'svelte';

	const policy = new Task<LanesPolicy>();
	const runs = new Task<{ runs: Run[] }>();
	const submission = new Task<Submission>();
	const classification = new Task<Classification>();
	const judgment = new Task<{ classification: Classification; judgment: Judgment }>();

	let base = $state(page.url.searchParams.get('base') ?? '');
	let head = $state(page.url.searchParams.get('head') ?? 'HEAD');
	let intent = $state('');
	let token = $state('');
	let evidence = $state<string[]>([]);
	let approvals = $state('');
	let escalations = $state('');
	let agent = $state({ tool: 'person', model: '', config: '', team: '' });

	onMount(() => {
		policy.run(() => api<LanesPolicy>('lanes.policy'));
		runs.run(() => api('evidence.list'));
		app.loadRefs().then((r) => {
			if (!base) base = r?.default ?? 'main';
		});
	});

	const lines = (s: string) => s.split('\n').map((l) => l.trim()).filter(Boolean);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		classification.reset();
		judgment.reset();
		const sub = await submission.run(() =>
			api<Submission>('lanes.submit', {
				base,
				head,
				intent: intent.trim() || undefined,
				evidence,
				token: token.trim() || undefined,
				approvals: lines(approvals),
				escalations: lines(escalations),
				agent
			})
		);
		if (sub) classification.run(() => api<Classification>('lanes.classify', { submission: sub }));
	}

	function judge() {
		const sub = submission.value;
		if (sub) judgment.run(() => api('lanes.judge', { submission: sub }));
	}

	const verdictTone = (v: string) => (v === 'approve' ? 'add' : v === 'reject' ? 'del' : 'signal');
	const stepTone = (s: string) =>
		s === 'passed' ? 'add' : s === 'failed' ? 'del' : s === 'concern' ? 'signal' : 'faint';
	const p = $derived(policy.value?.policy);
	const outcomeHref = $derived.by(() => {
		const j = judgment.value?.judgment;
		const sub = submission.value;
		if (!sub) return '/outcomes';
		const q = new URLSearchParams({
			change: short(sub.head, 12),
			agent: `${sub.agent.tool}/${sub.agent.model}/${sub.agent.config}`,
			lane: j?.lane ?? classification.value?.lane ?? 'judge',
			verdict: j?.verdict ?? '',
			judge: j?.judge ?? '',
			commit: sub.head
		});
		return `/outcomes?${q}`;
	});
</script>

<PageHead title="Lanes & judge" guide="lanes">
	Where a change goes: merge on its own, the verifying judge, a person, or nowhere. Rules decide, and floors no
	rule can lower; nothing about a lane is generated.
</PageHead>

<div class="stack">
	<Card title="Lane policy" subtitle={policy.value ? `Judge configuration ${policy.value.judge}` : undefined}>
		{#if policy.error}
			<ErrorBox error={policy.error} />
		{:else if !p}
			<Loading />
		{:else}
			{#if !policy.value?.configured}
				<p class="muted" style="margin-bottom: 12px">onus.yaml has no <code>lanes:</code> yet, so every change goes to the judge. A starting point:</p>
				<pre>{`lanes:
  default: judge
  auditRate: 0.05
  minRecord: 10
  rules:
    - { lane: auto-merge, match: every, kinds: [internal], components: [docs] }
    - { lane: human, subkinds: [migration-changed] }
  heldOut: { command: "npm run test:held-out", image: "node:22", setup: "npm ci" }`}</pre>
			{:else}
				<div class="policy">
					<div><span class="muted small">Default lane</span><div><LaneBadge lane={p.default ?? 'judge'} /></div></div>
					<div><span class="muted small">Audited auto-merges</span><div>{((p.auditRate ?? 0) * 100).toFixed(1)}%</div></div>
					<div><span class="muted small">Record before auto-merge</span><div>{p.minRecord ?? 0} merged changes</div></div>
					<div><span class="muted small">Held-out checks</span><div class="mono small">{p.heldOut?.command ?? 'none'}</div></div>
					<div><span class="muted small">Taste reviewer</span><div class="mono small">{p.taste?.command.join(' ') ?? 'none'}</div></div>
				</div>
				{#if p.rules?.length}
					<table class="data" style="margin-top: 12px">
						<thead><tr><th>Lane</th><th>Match</th><th>Kinds</th><th>Subkinds</th><th>Components</th><th>Labels</th></tr></thead>
						<tbody>
							{#each p.rules as r, i (i)}
								<tr>
									<td><LaneBadge lane={r.lane} /></td>
									<td>{r.match ?? 'any'}</td>
									<td class="small mono">{r.kinds?.join(', ') || 'any'}</td>
									<td class="small mono">{r.subkinds?.join(', ') || 'any'}</td>
									<td class="small mono">{r.components?.join(', ') || 'any'}</td>
									<td class="small mono">{r.labels?.join(', ') || 'any'}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
			{/if}
			<p class="small muted" style="margin-top: 12px">
				Floors: secrets, writes outside the token's scope and unapproved weakened tests are <strong>blocked</strong>;
				sensitive code, rules of the game and rows outside the intent go to <strong>a person</strong>.
			</p>
		{/if}
	</Card>

	<Card title="Submit a change" subtitle="What a reviewer needs and nothing of the author's reasoning.">
		<form class="stack" onsubmit={submit}>
			<div class="grid-2">
				<RefPicker id="lane-base" label="Base" bind:value={base} />
				<RefPicker id="lane-head" label="Head" bind:value={head} />
			</div>
			<label class="field"><span>Intent (YAML, or Markdown with an <code>onus-intent</code> block)</span>
				<textarea rows="4" bind:value={intent} placeholder={'summary: Redact digits in logs\ntouches: [logger]'}></textarea>
			</label>
			<div class="field">
				<span class="muted small" style="font-weight: 550">Evidence: runs recorded in environments</span>
				{#if runs.value?.runs.length}
					<div class="runs">
						{#each runs.value.runs.slice(0, 30) as r (r.id)}
							<label class="check run">
								<input type="checkbox" value={r.id} bind:group={evidence} />
								<span class="mono small">{short(r.id, 10)}</span>
								<Badge tone={r.manifest.exitCode === 0 ? 'add' : 'del'}>exit {r.manifest.exitCode}</Badge>
								<span class="mono small">{r.manifest.command}</span>
								<span class="faint small">at {short(r.manifest.commit)} · {ago(r.manifest.finishedAt)}</span>
							</label>
						{/each}
					</div>
				{:else}
					<p class="muted small">No runs yet. Record some in <a href="/environments">Environments</a>; the judge rejects a change without evidence.</p>
				{/if}
			</div>
			<div class="grid-2">
				<label class="field">Agent tool <input bind:value={agent.tool} placeholder="claude-code, codex, person…" /></label>
				<label class="field">Model <input bind:value={agent.model} /></label>
				<label class="field">Configuration <input bind:value={agent.config} /></label>
				<label class="field">Team <input bind:value={agent.team} /></label>
			</div>
			<details>
				<summary class="muted small">Token, approvals and escalations</summary>
				<div class="stack" style="margin-top: 12px">
					<label class="field">Task token (its verified scope is checked against every path written)<input class="mono" bind:value={token} /></label>
					<div class="grid-2">
						<label class="field"><span>Approvals, one <code>row id=who</code> per line</span><textarea rows="3" bind:value={approvals}></textarea></label>
						<label class="field">Granted escalations, one id per line<textarea rows="3" bind:value={escalations}></textarea></label>
					</div>
				</div>
			</details>
			<div class="row">
				<button class="primary" type="submit" disabled={submission.running || !base || !head}>
					{submission.running ? 'Reporting the change…' : 'Make the submission and classify'}
				</button>
			</div>
		</form>
	</Card>

	{#if submission.error}<ErrorBox error={submission.error} />{/if}

	{#if submission.value}
		{@const sub = submission.value}
		<div class="grid-2">
			<Card title="Lane">
				{#if classification.value}
					<ClassificationView classification={classification.value} />
				{:else if classification.error}
					<ErrorBox error={classification.error} />
				{:else}
					<Loading />
				{/if}
			</Card>
			<Card title="Submission">
				{#snippet actions()}
					<button class="small" onclick={() => download(`submission-${short(sub.head)}.json`, JSON.stringify(sub, null, 2))}>Download</button>
				{/snippet}
				<dl>
					<dt>Change</dt><dd class="mono">{short(sub.base)} → {short(sub.head)}</dd>
					<dt>Rows</dt><dd>{sub.report.changes.length} ({sub.report.summary.needsAttention} need a person)</dd>
					<dt>Files</dt><dd>{sub.changedFiles.length}</dd>
					<dt>Evidence</dt><dd>{sub.evidence.length} runs</dd>
					<dt>Scope</dt><dd>{sub.scope ? `${sub.scope.task}: ${sub.scope.rights.length} rights` : 'no token'}</dd>
					<dt>Agent</dt><dd class="mono small">{sub.agent.tool}/{sub.agent.model}/{sub.agent.config}</dd>
				</dl>
			</Card>
		</div>

		<Card title="The judge" subtitle="Verification first, taste last: re-runs the evidence in fresh containers, checks intent, contracts and tests, runs held-out checks.">
			{#snippet actions()}
				<button class="primary" onclick={judge} disabled={judgment.running}>{judgment.running ? 'Judging…' : 'Run the judge'}</button>
			{/snippet}
			{#if judgment.running}
				<Loading label="Re-running evidence and held-out checks in containers; this can take a few minutes…" />
			{:else if judgment.error}
				<ErrorBox error={judgment.error} />
			{:else if judgment.value}
				{@const j = judgment.value.judgment}
				<div class="stack">
					<div class="row">
						<span class="muted">Verdict</span>
						<span class="verdict"><Badge tone={verdictTone(j.verdict)}>{j.verdict}</Badge></span>
						<span class="muted small">in the <LaneBadge lane={j.lane} /> lane · judge {j.judge}</span>
						<a class="button small" href={outcomeHref}>Record the outcome</a>
					</div>
					{#if j.reasons.length}
						<ul class="reasons">{#each j.reasons as r (r)}<li><Inline text={r} /></li>{/each}</ul>
					{/if}
					<ol class="steps">
						{#each j.steps as s (s.name)}
							<li>
								<Badge tone={stepTone(s.status)}>{s.status}</Badge>
								<strong>{s.name}</strong>
								{#if s.details.length}<ul>{#each s.details as d (d)}<li class="small"><Inline text={d} /></li>{/each}</ul>{/if}
							</li>
						{/each}
					</ol>
				</div>
			{:else}
				<p class="muted small">The judge needs a container engine (Docker or Podman) to re-run the evidence.</p>
			{/if}
		</Card>

		<Card title="Rows" pad={false}>
			{#each sub.report.changes as c (c.id)}
				<ChangeRow change={c} />
			{:else}
				<div style="padding: 16px"><Empty title="No changes in meaning" /></div>
			{/each}
		</Card>
	{/if}
</div>

<style>
	.policy {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
		gap: var(--space-3);
	}
	.runs {
		display: grid;
		gap: 4px;
		max-height: 240px;
		overflow: auto;
		border: 1px solid var(--line);
		border-radius: var(--radius-s);
		padding: var(--space-2) var(--space-3);
	}
	.run {
		flex-wrap: wrap;
		color: var(--ink);
		font-weight: 400;
	}
	dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 6px var(--space-4);
		margin: 0;
	}
	dt {
		color: var(--ink-faint);
	}
	dd {
		margin: 0;
	}
	.verdict :global(.badge) {
		height: 26px;
		font-size: 13px;
		padding: 0 12px;
	}
	.reasons {
		margin: 0;
		padding-left: 1.2em;
		display: grid;
		gap: 4px;
	}
	.steps {
		margin: 0;
		padding-left: 1.2em;
		display: grid;
		gap: var(--space-3);
	}
	.steps ul {
		margin: 4px 0 0;
		color: var(--ink-muted);
	}
	details summary {
		cursor: pointer;
	}
</style>
